//! [`RedisCheckpoint`] — one delivery's view of its job's saved progress: a
//! field of the queue's `checkpoints` hash, written only while the delivery
//! still holds the job.

use async_trait::async_trait;
use nest_rs_queue::{CheckpointStore, QueueError};
use serde_json::Value;

use super::consumer::Lease;
use super::scripts::SCRIPTS;
use crate::RedisConnection;
use crate::error::CheckpointFenced;
use crate::layout::{GROUP, QueueKeys};

/// The checkpoint of the job one delivery runs, fenced on that delivery: a
/// worker whose lease lapsed under it writes nothing over the progress of the
/// delivery that took the job.
pub(crate) struct RedisCheckpoint {
    conn: RedisConnection,
    jobs: String,
    checkpoints: String,
    worker: String,
    lease: Lease,
    job: String,
}

impl RedisCheckpoint {
    pub(crate) fn new(
        conn: RedisConnection,
        keys: &QueueKeys,
        worker: String,
        lease: Lease,
        job: String,
    ) -> Self {
        Self {
            conn,
            jobs: keys.jobs.clone(),
            checkpoints: keys.checkpoints.clone(),
            worker,
            lease,
            job,
        }
    }

    /// Write `state` (`None`: clear it) while this delivery holds the job.
    async fn write(&self, state: Option<Vec<u8>>) -> Result<(), QueueError> {
        let operation = if state.is_some() { "save" } else { "clear" };
        let written: i64 = SCRIPTS
            .checkpoint
            .key(&self.jobs)
            .key(&self.checkpoints)
            .arg(GROUP)
            .arg(&self.worker)
            .arg(&self.lease.entry)
            .arg(self.lease.count)
            .arg(&self.job)
            .arg(operation)
            .arg(state.unwrap_or_default())
            .invoke_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        if written == 1 {
            Ok(())
        } else {
            Err(QueueError::backend(CheckpointFenced))
        }
    }
}

#[async_trait]
impl CheckpointStore for RedisCheckpoint {
    async fn load(&self) -> Result<Option<Value>, QueueError> {
        let saved: Option<Vec<u8>> = redis::cmd("HGET")
            .arg(&self.checkpoints)
            .arg(&self.job)
            .query_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        saved
            .map(|saved| serde_json::from_slice(&saved))
            .transpose()
            .map_err(QueueError::from)
    }

    async fn save(&self, state: Value) -> Result<(), QueueError> {
        self.write(Some(serde_json::to_vec(&state)?)).await
    }

    async fn clear(&self) -> Result<(), QueueError> {
        self.write(None).await
    }
}
