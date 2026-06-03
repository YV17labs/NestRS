use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use chrono_tz::Tz;
use croner::Cron;
use nestrs_core::{run_in_job_context, Container, DiscoveryService, JobContext, Transport};
use tokio::task::JoinSet;
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::{CronJobMeta, RunFn, Trigger};

/// Cron expressions and timezones are parsed in `configure` so a bad value
/// fails boot (not the first fire); each tick is cheap.
pub struct Scheduler {
    jobs: Vec<Job>,
    container: Option<Container>,
}

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
        // Boxed because a parsed Cron is ~330 bytes (large_enum_variant).
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
        // No jobs: idle until shutdown so this transport doesn't race the app
        // down when it is the only one attached.
        if self.jobs.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        // Resolve once: a database module's `WorkerDbContext` binds this so
        // each tick runs with a pool executor and the job queries through Repo.
        let ctx = container.get_dyn::<dyn JobContext>();

        let mut tasks = JoinSet::new();
        for job in self.jobs {
            let container = container.clone();
            let token = cancel.clone();
            let ctx = ctx.clone();
            tasks.spawn(async move { run_job(job, container, token, ctx).await });
        }
        while tasks.join_next().await.is_some() {}
        Ok(())
    }
}

/// Each variant computes its own waits; all return only when `token` is
/// cancelled (one-shot idles after its single run so the transport doesn't
/// race the app down).
async fn run_job(
    job: Job,
    container: Container,
    token: CancellationToken,
    ctx: Option<Arc<dyn JobContext>>,
) {
    let name = job.name();
    match job {
        Job::Interval { period, run, .. } => {
            let mut ticker = interval(period);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Drop the immediate first tick — semantics is "every N", not
            // "now, then every N".
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = ticker.tick() => fire(name, run, &container, &ctx).await,
                }
            }
        }
        Job::Timeout { delay, run, .. } => {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = sleep(delay) => fire(name, run, &container, &ctx).await,
            }
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
                _ = sleep(wait) => fire(name, run, &container, &ctx).await,
            }
        },
    }
}

/// `None` if the schedule has no future occurrence.
fn next_delay(schedule: &Cron, tz: Option<Tz>) -> Option<Duration> {
    let now = Utc::now();
    let next_utc = match tz {
        Some(tz) => schedule
            .find_next_occurrence(&now.with_timezone(&tz), false)
            .ok()
            .map(|dt| dt.with_timezone(&Utc)),
        None => schedule.find_next_occurrence(&now, false).ok(),
    }?;
    // `find_next_occurrence(.., false)` is strictly after `now`; clamp
    // defensively rather than unwrap a negative span.
    Some((next_utc - now).to_std().unwrap_or(Duration::ZERO))
}

async fn fire(
    name: &'static str,
    run: RunFn,
    container: &Container,
    ctx: &Option<Arc<dyn JobContext>>,
) {
    let result = run_in_job_context(ctx.as_ref(), run(container)).await;
    if let Err(err) = result {
        tracing::error!(
            target: "nestrs::schedule",
            job = name,
            error = %err,
            "scheduled job failed",
        );
    }
}
