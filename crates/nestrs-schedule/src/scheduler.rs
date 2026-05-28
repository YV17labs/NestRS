use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use chrono_tz::Tz;
use croner::Cron;
use nestrs_core::{Container, DiscoveryService, Transport};
use tokio::task::JoinSet;
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::{CronJobMeta, RunFn, Trigger};

/// A [`Transport`] that runs every `#[cron_job]` discovered in the module tree.
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
/// fully-assembled container and resolves it into a runnable [`Job`] — parsing a
/// cron expression and its timezone here, so a malformed value **fails the boot**
/// with a message naming the job. [`serve`](Transport::serve) spawns one task per
/// job and runs it until shutdown is signalled. Each run rebuilds the job from the
/// container; an error is logged and the schedule continues.
pub struct Scheduler {
    jobs: Vec<Job>,
    container: Option<Container>,
}

/// A [`CronJobMeta`] resolved into something runnable: the cron expression and
/// timezone are parsed once at configure time so each tick is cheap and a bad
/// value is caught at boot rather than on the first fire.
enum Job {
    Interval {
        name: &'static str,
        period: Duration,
        run: RunFn,
    },
    Timeout {
        name: &'static str,
        delay: Duration,
        run: RunFn,
    },
    Cron {
        name: &'static str,
        // Boxed: a parsed `Cron` is large (~330 bytes), and inlining it would
        // bloat every `Job` to that size — `large_enum_variant`.
        schedule: Box<Cron>,
        tz: Option<Tz>,
        run: RunFn,
    },
}

impl Job {
    fn name(&self) -> &'static str {
        match self {
            Job::Interval { name, .. } | Job::Timeout { name, .. } | Job::Cron { name, .. } => name,
        }
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            container: None,
        }
    }

    /// Resolve a discovered meta into a runnable [`Job`], parsing a cron
    /// expression and timezone up front. An invalid value is a boot failure.
    fn resolve(meta: &Arc<CronJobMeta>) -> Result<Job> {
        let name = meta.name;
        Ok(match meta.trigger {
            Trigger::Interval(period) => Job::Interval {
                name,
                period,
                run: meta.run,
            },
            Trigger::Timeout(delay) => Job::Timeout {
                name,
                delay,
                run: meta.run,
            },
            Trigger::Cron { expr, tz } => {
                let schedule = Cron::from_str(expr).with_context(|| {
                    format!("cron job `{name}` has an invalid cron expression `{expr}`")
                })?;
                let tz = tz
                    .map(|name_str| {
                        name_str.parse::<Tz>().map_err(|e| {
                            anyhow::anyhow!(
                                "cron job `{name}` has an invalid timezone `{name_str}`: {e}"
                            )
                        })
                    })
                    .transpose()?;
                Job::Cron {
                    name,
                    schedule: Box::new(schedule),
                    tz,
                    run: meta.run,
                }
            }
        })
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
            .iter()
            .map(|d| Scheduler::resolve(&d.meta))
            .collect::<Result<Vec<_>>>()?;
        for job in &self.jobs {
            match job {
                Job::Interval { name, period, .. } => tracing::info!(
                    target: "nestrs::schedule",
                    job = name,
                    interval_ms = period.as_millis() as u64,
                    "scheduled job (interval)",
                ),
                Job::Timeout { name, delay, .. } => tracing::info!(
                    target: "nestrs::schedule",
                    job = name,
                    delay_ms = delay.as_millis() as u64,
                    "scheduled job (one-shot)",
                ),
                Job::Cron { name, tz, .. } => tracing::info!(
                    target: "nestrs::schedule",
                    job = name,
                    timezone = tz.map(|t| t.name()).unwrap_or("UTC"),
                    "scheduled job (cron)",
                ),
            }
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
            tasks.spawn(async move { run_job(job, container, token).await });
        }
        while tasks.join_next().await.is_some() {}
        Ok(())
    }
}

/// Drive a single job until cancellation. Each variant computes its own waits;
/// all of them return only when `token` is cancelled (a one-shot idles after its
/// single run so the transport doesn't race the app down).
async fn run_job(job: Job, container: Container, token: CancellationToken) {
    let name = job.name();
    match job {
        Job::Interval { period, run, .. } => {
            let mut ticker = interval(period);
            // A skipped slow tick must not burst-fire to catch up.
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Drop the immediate first tick so the first run lands one period in,
            // matching "every N" rather than "now, then every N".
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = ticker.tick() => fire(name, run, &container).await,
                }
            }
        }
        Job::Timeout { delay, run, .. } => {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = sleep(delay) => fire(name, run, &container).await,
            }
            // Run once, then idle until shutdown.
            token.cancelled().await;
        }
        Job::Cron {
            schedule, tz, run, ..
        } => loop {
            let wait = match next_delay(&schedule, tz) {
                Some(d) => d,
                None => {
                    tracing::warn!(
                        target: "nestrs::schedule",
                        job = name,
                        "cron job has no future occurrence; it will not run again",
                    );
                    token.cancelled().await;
                    break;
                }
            };
            tokio::select! {
                _ = token.cancelled() => break,
                _ = sleep(wait) => fire(name, run, &container).await,
            }
        },
    }
}

/// The duration from now until the schedule's next occurrence, in the job's
/// timezone (UTC when unset). `None` if the schedule has no future occurrence.
fn next_delay(schedule: &Cron, tz: Option<Tz>) -> Option<Duration> {
    let now = Utc::now();
    let next_utc = match tz {
        Some(tz) => schedule
            .find_next_occurrence(&now.with_timezone(&tz), false)
            .ok()
            .map(|dt| dt.with_timezone(&Utc)),
        None => schedule.find_next_occurrence(&now, false).ok(),
    }?;
    // `find_next_occurrence(.., false)` is strictly after `now`, so the delta is
    // positive; clamp to zero defensively rather than unwrap a negative span.
    Some((next_utc - now).to_std().unwrap_or(Duration::ZERO))
}

async fn fire(name: &'static str, run: RunFn, container: &Container) {
    if let Err(err) = run(container).await {
        tracing::error!(
            target: "nestrs::schedule",
            job = name,
            error = %err,
            "scheduled job failed",
        );
    }
}
