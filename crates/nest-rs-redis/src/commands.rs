//! Every command a connection sends, handed to the observability stack as the
//! client operation the database conventions name for Redis — the command's
//! name, never a key or an argument — once the server answered it.

use nest_rs_core::client_operation::{
    self, ClientValue, DB_CLIENT_OPERATION_DURATION, DB_NAMESPACE, DB_OPERATION_NAME,
    DB_SYSTEM_NAME, DatabaseFailure, PendingClientOperation, SERVER_ADDRESS, SERVER_PORT,
};
use nest_rs_core::operation_log::OTHER;
use redis::{Arg, Cmd, Pipeline, RedisError, Value};

/// The database conventions' size of a batch, which no duration is keyed by.
const DB_OPERATION_BATCH_SIZE: &str = "db.operation.batch.size";

/// The conventions' name for the database this driver speaks to.
const REDIS: &str = "redis";

/// The conventions' operation word for several commands sent at once.
const BATCH: &str = "BATCH";

/// The longest command name or error code read as one: anything longer, or
/// holding anything but letters, digits and `_`, reads as `_OTHER`.
const LONGEST_WORD: usize = 32;

/// What was sent: one command, or a pipeline of `size` of them, each the
/// command `operation` when they all are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sent<'a> {
    Command(&'a str),
    Batch {
        operation: Option<&'a str>,
        size: usize,
    },
}

impl<'a> Sent<'a> {
    /// What `cmd` is named.
    pub(crate) fn command(cmd: &'a Cmd) -> Self {
        Self::Command(name(cmd))
    }

    /// What the `count` commands of `pipeline` sent from `offset` are named: a
    /// batch of two or more, the command itself for one, as the database
    /// conventions count a batch.
    pub(crate) fn batch(pipeline: &'a Pipeline, offset: usize, count: usize) -> Self {
        // redis-rs sends a transaction as MULTI, its commands and EXEC, and
        // asks for EXEC's answer alone: its offset is past every command.
        let (offset, count) = if pipeline.is_transaction() {
            (0, pipeline.cmd_iter().count())
        } else {
            (offset, count)
        };
        let mut names = pipeline.cmd_iter().skip(offset).take(count).map(name);
        let first = names.next();
        if count == 1 {
            return Self::Command(first.unwrap_or(OTHER));
        }
        let operation = first.filter(|first| names.all(|name| name == *first));
        Self::Batch {
            operation,
            size: count,
        }
    }

    /// The operation's name: the command, `BATCH <command>`, or `BATCH`.
    fn operation(&self) -> String {
        match self {
            Self::Command(command) => (*command).to_owned(),
            Self::Batch {
                operation: Some(operation),
                ..
            } => format!("{BATCH} {operation}"),
            Self::Batch {
                operation: None, ..
            } => BATCH.to_owned(),
        }
    }
}

/// A command's name, read off its first argument as the server reads it.
fn name(cmd: &Cmd) -> &str {
    match cmd.args_iter().next() {
        Some(Arg::Simple(bytes)) => word(bytes),
        _ => OTHER,
    }
}

/// `bytes` as a word a series may be keyed by, or `_OTHER`.
fn word(bytes: &[u8]) -> &str {
    let is_word = !bytes.is_empty()
        && bytes.len() <= LONGEST_WORD
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
    match std::str::from_utf8(bytes) {
        Ok(word) if is_word => word,
        _ => OTHER,
    }
}

/// What `sent` names, about to leave for the database `namespace` of `server`:
/// pending until it answers, or until its caller gives up on it — untimed, and
/// `sent` never read, where nothing would record it.
pub(crate) fn sending<'s>(
    sent: impl FnOnce() -> Sent<'s>,
    namespace: &str,
    server: Option<(&str, u16)>,
) -> PendingClientOperation {
    if !client_operation::is_recorded_here() {
        return PendingClientOperation::untimed();
    }
    let sent = sent();
    let operation = sent.operation();
    let mut attributes = Vec::with_capacity(6);
    attributes.push((DB_SYSTEM_NAME, ClientValue::Text(REDIS)));
    attributes.push((DB_NAMESPACE, ClientValue::Text(namespace)));
    attributes.push((DB_OPERATION_NAME, ClientValue::Text(&operation)));
    if let Sent::Batch { size, .. } = sent {
        attributes.push((
            DB_OPERATION_BATCH_SIZE,
            ClientValue::Integer(i64::try_from(size).unwrap_or(i64::MAX)),
        ));
    }
    if let Some((address, port)) = server {
        attributes.push((SERVER_ADDRESS, ClientValue::Text(address)));
        attributes.push((SERVER_PORT, ClientValue::Integer(i64::from(port))));
    }
    PendingClientOperation::start(
        &DB_CLIENT_OPERATION_DURATION,
        operation.clone(),
        &attributes,
    )
}

/// How a command's `answer` ended: the code the server refused it with —
/// redis-rs hands a refusal back as a value, or as an error carrying it — a
/// budget run out, or no answer.
pub(crate) fn command_answer(
    answer: &Result<Value, RedisError>,
) -> Result<(), DatabaseFailure<'_>> {
    match answer {
        Ok(value) => refusal(value).map_or(Ok(()), |code| Err(DatabaseFailure::Refused(code))),
        Err(error) => Err(failure(error)),
    }
}

/// How `pipeline`'s `answers` ended: by the first refusal among them — inside
/// `EXEC`'s answer for a transaction, which answers each command it ran there.
pub(crate) fn batch_answer<'v>(
    pipeline: &Pipeline,
    answers: &'v Result<Vec<Value>, RedisError>,
) -> Result<(), DatabaseFailure<'v>> {
    let refused = match answers {
        Ok(values) if pipeline.is_transaction() => values.iter().find_map(|value| match value {
            Value::Array(ran) => ran.iter().find_map(refusal),
            answered => refusal(answered),
        }),
        Ok(values) => values.iter().find_map(refusal),
        Err(error) => return Err(failure(error)),
    };
    refused.map_or(Ok(()), |code| Err(DatabaseFailure::Refused(code)))
}

fn refusal(value: &Value) -> Option<&str> {
    match value {
        Value::ServerError(refused) => Some(word(refused.code().as_bytes())),
        _ => None,
    }
}

fn failure(error: &RedisError) -> DatabaseFailure<'_> {
    if error.is_timeout() {
        return DatabaseFailure::TimedOut;
    }
    error.code().map_or(DatabaseFailure::Unanswered, |code| {
        DatabaseFailure::Refused(word(code.as_bytes()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_named_by_its_first_word_and_never_by_its_keys() {
        assert_eq!(
            Sent::command(redis::cmd("GET").arg("session:alice")),
            Sent::Command("GET"),
        );
        assert_eq!(
            Sent::command(redis::cmd("EVALSHA").arg("0123abcd").arg(1).arg("k")),
            Sent::Command("EVALSHA"),
        );
    }

    #[test]
    fn a_word_no_series_may_hold_reads_as_other() {
        assert_eq!(word(b"GET"), "GET");
        assert_eq!(word(b"CLIENT"), "CLIENT");
        assert_eq!(word(b"a key with spaces"), OTHER);
        assert_eq!(word(&[b'X'; LONGEST_WORD + 1]), OTHER);
        assert_eq!(word(&[0xff, 0xfe]), OTHER);
        assert_eq!(word(b""), OTHER);
    }

    #[test]
    fn a_command_whose_budget_ran_out_reads_as_a_timeout_and_names_no_server_code() {
        let ran_out = RedisError::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the budget ran out",
        ));
        assert_eq!(
            command_answer(&Err(ran_out)),
            Err(DatabaseFailure::TimedOut)
        );
    }

    #[test]
    fn a_refusal_the_server_answers_with_is_its_code() {
        let refused = || -> Value {
            redis::parse_redis_value(b"-NOSCRIPT No matching script.\r\n")
                .expect("a refusal, as the server writes one")
        };
        assert_eq!(
            command_answer(&Ok(refused())),
            Err(DatabaseFailure::Refused("NOSCRIPT"))
        );
        assert_eq!(command_answer(&Ok(Value::Okay)), Ok(()));
        let pipeline = redis::pipe();
        assert_eq!(
            batch_answer(&pipeline, &Ok(vec![Value::Okay, refused(), Value::Nil])),
            Err(DatabaseFailure::Refused("NOSCRIPT")),
        );
        assert_eq!(batch_answer(&pipeline, &Ok(vec![Value::Okay])), Ok(()));
    }

    /// redis-rs sends a transaction as `MULTI`, its commands and `EXEC`, and
    /// asks for one answer past every command: `EXEC`'s.
    fn sent_as_redis_rs_sends(pipeline: &Pipeline) -> Sent<'_> {
        let commands = pipeline.cmd_iter().count();
        if pipeline.is_transaction() {
            Sent::batch(pipeline, commands + 1, 1)
        } else {
            Sent::batch(pipeline, 0, commands)
        }
    }

    #[test]
    fn a_transaction_is_a_batch_of_the_commands_it_runs() {
        let mut transaction = redis::pipe();
        transaction.atomic().set("a", 1).set("b", 2);
        assert_eq!(
            sent_as_redis_rs_sends(&transaction),
            Sent::Batch {
                operation: Some("SET"),
                size: 2,
            },
        );
    }

    #[test]
    fn a_pipeline_of_one_command_is_that_command() {
        let mut one = redis::pipe();
        one.set("a", 1);
        assert_eq!(sent_as_redis_rs_sends(&one), Sent::Command("SET"));
        one.atomic();
        assert_eq!(sent_as_redis_rs_sends(&one), Sent::Command("SET"));
    }

    #[test]
    fn a_refusal_inside_a_transactions_answer_is_its_code() {
        let wrongtype = redis::parse_redis_value(
            b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n",
        )
        .expect("a refusal, as the server writes one");
        let mut transaction = redis::pipe();
        transaction.atomic().set("a", 1).lpush("a", 2);
        assert_eq!(
            batch_answer(
                &transaction,
                &Ok(vec![Value::Array(vec![Value::Okay, wrongtype])]),
            ),
            Err(DatabaseFailure::Refused("WRONGTYPE")),
        );
    }

    #[test]
    fn a_pipeline_is_a_batch_named_for_its_command_when_every_one_is_it() {
        let mut same = redis::pipe();
        same.cmd("SET").arg("a").arg(1).cmd("SET").arg("b").arg(2);
        assert_eq!(
            Sent::batch(&same, 0, 2),
            Sent::Batch {
                operation: Some("SET"),
                size: 2,
            },
        );
        assert_eq!(Sent::batch(&same, 0, 2).operation(), "BATCH SET");

        let mut mixed = redis::pipe();
        mixed.cmd("MULTI").cmd("SET").arg("a").arg(1).cmd("EXEC");
        let sent = Sent::batch(&mixed, 0, 3);
        assert_eq!(
            sent,
            Sent::Batch {
                operation: None,
                size: 3,
            },
        );
        assert_eq!(sent.operation(), BATCH);
        assert_eq!(
            Sent::batch(&mixed, 1, 2),
            Sent::Batch {
                operation: None,
                size: 2,
            },
            "the commands the pipeline sends from its offset",
        );
    }
}
