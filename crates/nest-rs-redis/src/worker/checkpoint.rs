//! [`RedisCheckpoint`] — one job's `Checkpoint` in Redis, under
//! `nestrs:queue:<queue>:checkpoints:<job_id>`.
//!
//! The port reads the state once per delivery, remembers every save made through
//! it and clears it when the job reaches its terminal outcome; this is the
//! storage under that, and nothing more. A save is written at once — outside
//! any transaction, which is why the port refuses a `Checkpoint` beside a
//! transactional method — and kept for [`KEPT_PAST_DUE`], renewed by every save
//! and every delivery of the job, so a checkpoint whose job vanished does not
//! outlive it for long. A cancel removes it with the job's other records.

use async_trait::async_trait;
use nest_rs_queue::{CheckpointStore, JobId, QueueError, QueueName};
use serde_json::Value;

use crate::RedisConnection;
use crate::layout::{CHECKPOINTS, KEPT_PAST_DUE, job_key, millis};

/// One job's checkpoint, over the shared connection.
pub(crate) struct RedisCheckpoint {
    conn: RedisConnection,
    key: String,
}

impl RedisCheckpoint {
    /// The checkpoint of `job` on `queue`.
    pub(crate) fn new(conn: RedisConnection, queue: &QueueName, job: &JobId) -> Self {
        Self {
            conn,
            key: job_key(CHECKPOINTS, queue, job),
        }
    }
}

#[async_trait]
impl CheckpointStore for RedisCheckpoint {
    async fn load(&self) -> Result<Option<Value>, QueueError> {
        let saved: Option<String> = redis::cmd("GET")
            .arg(&self.key)
            .query_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        Ok(saved.map(decoded))
    }

    async fn save(&self, state: Value) -> Result<(), QueueError> {
        redis::cmd("SET")
            .arg(&self.key)
            .arg(state.to_string())
            .arg("PX")
            .arg(millis(KEPT_PAST_DUE))
            .query_async::<()>(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)
    }

    async fn clear(&self) -> Result<(), QueueError> {
        redis::cmd("DEL")
            .arg(&self.key)
            .query_async::<i64>(&mut self.conn.clone())
            .await
            .map(drop)
            .map_err(QueueError::backend)
    }
}

/// The state a save stored. Bytes that are not JSON — written by anything but a
/// save — are handed on as the string they are, so the port's decode refuses
/// them as the deterministic failure they are: an error here would read as Redis
/// unreachable, and every attempt would retry into the same bytes.
fn decoded(saved: String) -> Value {
    serde_json::from_str(&saved).unwrap_or(Value::String(saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save round-trips as the value it stored; foreign bytes reach the port
    /// as a string its decode refuses, never as a failure to read.
    #[test]
    fn a_saved_state_reads_back_and_foreign_bytes_read_as_a_string() {
        assert_eq!(
            decoded(r#"{"row":7}"#.to_owned()),
            serde_json::json!({ "row": 7 })
        );
        assert_eq!(decoded("7".to_owned()), serde_json::json!(7));
        assert_eq!(
            decoded("not json".to_owned()),
            Value::String("not json".to_owned())
        );
    }
}
