//! Shared Redis connection + typed [`Queue`] producer handle. The queue name
//! supplied at the call site must match the consuming `#[processor(queue = ...)]`.

use apalis::prelude::Storage;
use apalis_redis::{Config, ConnectionManager, RedisStorage};

use crate::processor::Job;

#[derive(Clone)]
pub struct QueueConnection {
    conn: ConnectionManager,
}

impl QueueConnection {
    pub async fn connect(redis_url: &str) -> anyhow::Result<Self> {
        let conn = apalis_redis::connect(redis_url).await?;
        Ok(Self { conn })
    }

    pub fn of<J: Job>(&self, queue: &str) -> Queue<J> {
        Queue {
            storage: self.storage(queue),
        }
    }

    pub(crate) fn storage<J: Job>(&self, queue: &str) -> RedisStorage<J> {
        RedisStorage::new_with_config(self.conn.clone(), Config::default().set_namespace(queue))
    }

    /// Fetch buffer = processor concurrency — the in-flight-job ceiling.
    pub(crate) fn consumer_storage<J: Job>(
        &self,
        queue: &str,
        concurrency: usize,
    ) -> RedisStorage<J> {
        RedisStorage::new_with_config(
            self.conn.clone(),
            Config::default()
                .set_namespace(queue)
                .set_buffer_size(concurrency.max(1)),
        )
    }
}

pub struct Queue<J: Job> {
    storage: RedisStorage<J>,
}

impl<J: Job> Queue<J> {
    pub async fn push(&self, job: J) -> anyhow::Result<()> {
        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so clone per call rather than force callers to hold it mut.
        let mut storage = self.storage.clone();
        storage.push(job).await?;
        Ok(())
    }
}
