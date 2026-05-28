use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use nestrs_core::{Container, DiscoveryService, Transport};
use tokio::task::JoinSet;
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::CronJobMeta;

/// A [`Transport`] that ticks every `#[cron_job]` discovered in the module tree.
/// Attach it in `main` alongside other transports:
///
/// ```ignore
/// App::new::<AppModule>()?
///     .transport(Scheduler::new())
///     .transport(HttpTransport::new())
///     .run().await
/// ```
///
/// At [`configure`](Transport::configure) it reads every [`CronJobMeta`] from the
/// fully-assembled container; [`serve`](Transport::serve) spawns one task per job
/// that ticks on the job's period until shutdown is signalled. Each tick rebuilds
/// the job from the container and runs it; an error is logged and the schedule
/// continues.
pub struct Scheduler {
    jobs: Vec<Arc<CronJobMeta>>,
    container: Option<Container>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            container: None,
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for Scheduler {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        let discovery = DiscoveryService::new(container);
        self.jobs = discovery
            .meta::<CronJobMeta>()
            .into_iter()
            .map(|d| d.meta)
            .collect();
        for job in &self.jobs {
            tracing::info!(
                target: "nestrs::schedule",
                job = job.name,
                period_ms = job.period.as_millis() as u64,
                "scheduled job",
            );
        }
        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        let container = self
            .container
            .expect("Scheduler::configure must run before serve");
        // No jobs: idle until shutdown rather than returning, so this transport
        // doesn't race the app down when it is the only one attached.
        if self.jobs.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        let mut tasks = JoinSet::new();
        for job in self.jobs {
            let container = container.clone();
            let token = cancel.clone();
            tasks.spawn(async move {
                let mut ticker = interval(job.period);
                // A skipped slow tick must not burst-fire to catch up.
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                // Drop the immediate first tick so the first run lands one period
                // in, matching "every N" rather than "now, then every N".
                ticker.tick().await;
                loop {
                    tokio::select! {
                        _ = token.cancelled() => break,
                        _ = ticker.tick() => {
                            if let Err(err) = (job.run)(&container).await {
                                tracing::error!(
                                    target: "nestrs::schedule",
                                    job = job.name,
                                    error = %err,
                                    "scheduled job failed",
                                );
                            }
                        }
                    }
                }
            });
        }
        while tasks.join_next().await.is_some() {}
        Ok(())
    }
}
