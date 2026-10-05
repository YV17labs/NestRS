use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use nest_rs::queue::{JobProducer, JobProducerExt};
use tokio::task::JoinSet;

use crate::command::{C1Queue, C16Queue, NoopCommand, R16Queue};
use crate::probe::now_us;

/// The two queues of the bench's worker, one per `#[process]` concurrency.
#[derive(Clone, Copy, Debug)]
pub enum Lane {
    C1,
    C16,
    R16,
}

impl Lane {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "c1" => Ok(Self::C1),
            "c16" => Ok(Self::C16),
            "r16" => Ok(Self::R16),
            _ => bail!("--queue takes c1, c16 or r16, not `{raw}`"),
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::C1 => "`bench-c1` (concurrency 1)",
            Self::C16 => "`bench-c16` (concurrency 16)",
            Self::R16 => "`bench-r16` (concurrency 16, 3 retries)",
        }
    }

    /// Push job `seq`, `pad` numbers of ballast, stamped with the instant just
    /// before the call.
    pub async fn push(self, producer: &dyn JobProducer, seq: u32, pad: usize) -> Result<()> {
        let job = NoopCommand {
            seq,
            pushed_us: now_us(),
            pad: vec![0; pad],
        };
        match self {
            Self::C1 => producer.push(C1Queue, job, None).await?,
            Self::C16 => producer.push(C16Queue, job, None).await?,
            Self::R16 => producer.push(R16Queue, job, None).await?,
        };
        Ok(())
    }

    /// Push jobs `0..jobs`, one call each, from `pushers` concurrent tasks.
    pub async fn push_all(
        self,
        producer: &Arc<dyn JobProducer>,
        jobs: u32,
        pushers: u32,
        pad: usize,
    ) -> Result<Duration> {
        let started = Instant::now();
        let mut tasks = JoinSet::new();
        for first in 0..pushers.min(jobs) {
            let producer = Arc::clone(producer);
            tasks.spawn(async move {
                let mut seq = first;
                while seq < jobs {
                    self.push(&*producer, seq, pad).await?;
                    seq += pushers;
                }
                anyhow::Ok(())
            });
        }
        while let Some(pushed) = tasks.join_next().await {
            pushed??;
        }
        Ok(started.elapsed())
    }
}
