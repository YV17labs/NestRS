//! Every command a connection sends, handed to the observability stack as the
//! client operation the database conventions name for Redis — the command's
//! name, never a key or an argument — once the server answered it.

use std::time::{Instant, SystemTime};

use nest_rs_core::client_operation::{
    self, ClientOperation, ClientValue, DB_CLIENT_OPERATION_DURATION, DB_NAMESPACE,
    DB_OPERATION_NAME, DB_SYSTEM_NAME, SERVER_ADDRESS, SERVER_PORT,
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

    /// What `count` commands of `pipeline`, from `offset`, are named.
    pub(crate) fn batch(pipeline: &'a Pipeline, offset: usize, count: usize) -> Self {
        let mut names = pipeline.cmd_iter().skip(offset).take(count).map(name);
        let first = names.next();
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

/// A command being timed, from the instant it was sent.
pub(crate) struct Timing {
    started: SystemTime,
    clock: Instant,
}

impl Timing {
    /// Started now — where anything records client operations.
    pub(crate) fn start() -> Option<Self> {
        client_operation::is_recorded().then(|| Self {
            started: SystemTime::now(),
            clock: Instant::now(),
        })
    }

    /// Record `sent`, on the database `db` of `server`, as it answered:
    /// failed with the class `failure` names, if any.
    pub(crate) fn record(
        self,
        sent: Sent<'_>,
        db: i64,
        server: Option<(&str, u16)>,
        failure: Option<&str>,
    ) {
        let elapsed = self.clock.elapsed();
        let operation = sent.operation();
        let namespace = db.to_string();
        let mut attributes = Vec::with_capacity(6);
        attributes.push((DB_SYSTEM_NAME, ClientValue::Text(REDIS)));
        attributes.push((DB_NAMESPACE, ClientValue::Text(&namespace)));
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
        ClientOperation::new(
            &DB_CLIENT_OPERATION_DURATION,
            &operation,
            self.started,
            elapsed,
            &attributes,
        )
        .failed(failure)
        .record();
    }
}

/// How a command's `answer` failed: the code the server refused it with —
/// redis-rs hands a refusal back as a value, never as an error — or `_OTHER`
/// for an answer that never came.
pub(crate) fn command_failure(answer: &Result<Value, RedisError>) -> Option<&str> {
    match answer {
        Ok(value) => refusal(value),
        Err(error) => Some(error_class(error)),
    }
}

/// How a pipeline's `answers` failed: by the first refusal among them.
pub(crate) fn batch_failure(answers: &Result<Vec<Value>, RedisError>) -> Option<&str> {
    match answers {
        Ok(values) => values.iter().find_map(refusal),
        Err(error) => Some(error_class(error)),
    }
}

fn refusal(value: &Value) -> Option<&str> {
    match value {
        Value::ServerError(refused) => Some(word(refused.code().as_bytes())),
        _ => None,
    }
}

fn error_class(error: &RedisError) -> &str {
    error.code().map_or(OTHER, |code| word(code.as_bytes()))
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
    fn a_refusal_the_server_answers_with_is_its_code() {
        let refused = || -> Value {
            redis::parse_redis_value(b"-NOSCRIPT No matching script.\r\n")
                .expect("a refusal, as the server writes one")
        };
        assert_eq!(command_failure(&Ok(refused())), Some("NOSCRIPT"));
        assert_eq!(command_failure(&Ok(Value::Okay)), None);
        assert_eq!(
            batch_failure(&Ok(vec![Value::Okay, refused(), Value::Nil])),
            Some("NOSCRIPT"),
        );
        assert_eq!(batch_failure(&Ok(vec![Value::Okay])), None);
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
            Sent::batch(&mixed, 1, 1),
            Sent::Batch {
                operation: Some("SET"),
                size: 1,
            },
            "the commands the pipeline sends from its offset",
        );
    }
}
