use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use chrono_tz::Tz;
use croner::Cron;
use nest_rs_core::{Container, DiscoveryService, ReachableProviders, Transport, inventory};
use nest_rs_worker::{JobContext, run_in_job_context};
use tokio::task::JoinSet;
use tokio::time::{MissedTickBehavior, interval, sleep};
use tokio_util::sync::CancellationToken;

use crate::{CronJobMeta, RunFn, ScheduledMethod, Trigger};

/// Cron expressions and timezones are parsed in `configure` so a bad value
/// fails boot (not the first fire); each tick is cheap.
pub struct Scheduler {
    jobs: Vec<Job>,
    container: Option<Container>,
}

enum Job {
    Interval {
        id: JobId,
        period: Duration,
        run: RunFn,
    },
    Timeout {
        id: JobId,
        delay: Duration,
        run: RunFn,
    },
    Cron {
        id: JobId,
        // Boxed because a parsed Cron is ~330 bytes (large_enum_variant).
        schedule: Box<Cron>,
        tz: Option<Tz>,
        run: RunFn,
    },
}

/// A job's identity, kept split (host struct + method) so logs filter on
/// either field alone instead of parsing a baked `Provider::method` string.
#[derive(Clone, Copy)]
struct JobId {
    provider: &'static str,
    method: &'static str,
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.provider, self.method)
    }
}

impl Job {
    fn id(&self) -> JobId {
        match self {
            Job::Interval { id, .. } | Job::Timeout { id, .. } | Job::Cron { id, .. } => *id,
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
        let id = JobId {
            provider: meta.provider,
            method: meta.method,
        };
        Ok(match meta.trigger {
            Trigger::Interval(period) => Job::Interval {
                id,
                period,
                run: meta.run,
            },
            Trigger::Timeout(delay) => Job::Timeout {
                id,
                delay,
                run: meta.run,
            },
            Trigger::Cron { expr, tz } => {
                let schedule = Cron::from_str(expr).with_context(|| {
                    format!("cron job `{id}` has an invalid cron expression `{expr}`")
                })?;
                let tz = tz
                    .map(|name_str| {
                        name_str.parse::<Tz>().map_err(|e| {
                            anyhow::anyhow!(
                                "cron job `{id}` has an invalid timezone `{name_str}`: {e}"
                            )
                        })
                    })
                    .transpose()?;
                Job::Cron {
                    id,
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
        // Path 1: `attach_meta::<…, CronJobMeta>` — direct registration the
        // crate's own tests use; also a hand-written escape hatch for an app
        // that wants to register a job without going through the macros.
        let mut jobs: Vec<Job> = discovery
            .meta::<CronJobMeta>()
            .iter()
            .map(|d| Scheduler::resolve(&d.meta))
            .collect::<Result<Vec<_>>>()?;
        // Path 2: link-time inventory from `#[scheduled]` — module-gated by
        // `ReachableProviders` so a job whose provider lives in an unimported
        // module compiles in but does not fire.
        let reachable = container.get::<ReachableProviders>();
        for entry in inventory::iter::<ScheduledMethod>() {
            let provider_id = (entry.provider_type_id)();
            if let Some(r) = reachable.as_ref()
                && !r.0.contains(&provider_id)
            {
                tracing::debug!(
                    target: "nest_rs::schedule",
                    provider = entry.provider,
                    method = entry.method,
                    "skipped scheduled method: provider unreachable from app's module tree",
                );
                continue;
            }
            let synthesized = Arc::new(CronJobMeta {
                provider: entry.provider,
                method: entry.method,
                trigger: entry.trigger,
                run: entry.run,
            });
            jobs.push(Scheduler::resolve(&synthesized)?);
        }
        self.jobs = jobs;
        for job in &self.jobs {
            match job {
                Job::Interval { id, period, .. } => tracing::info!(
                    target: "nest_rs::schedule",
                    provider = id.provider,
                    method = id.method,
                    interval_ms = period.as_millis() as u64,
                    "scheduled job (interval)",
                ),
                Job::Timeout { id, delay, .. } => tracing::info!(
                    target: "nest_rs::schedule",
                    provider = id.provider,
                    method = id.method,
                    delay_ms = delay.as_millis() as u64,
                    "scheduled job (one-shot)",
                ),
                Job::Cron { id, tz, .. } => tracing::info!(
                    target: "nest_rs::schedule",
                    provider = id.provider,
                    method = id.method,
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
    let id = job.id();
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
                    _ = ticker.tick() => fire(id, run, &container, &ctx).await,
                }
            }
        }
        Job::Timeout { delay, run, .. } => {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = sleep(delay) => fire(id, run, &container, &ctx).await,
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
                        target: "nest_rs::schedule",
                        provider = id.provider,
                        method = id.method,
                        "cron job has no future occurrence; it will not run again",
                    );
                    token.cancelled().await;
                    break;
                }
            };
            tokio::select! {
                _ = token.cancelled() => break,
                _ = sleep(wait) => fire(id, run, &container, &ctx).await,
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
    id: JobId,
    run: RunFn,
    container: &Container,
    ctx: &Option<Arc<dyn JobContext>>,
) {
    let result = run_in_job_context(ctx.as_ref(), run(container)).await;
    if let Err(err) = result {
        tracing::error!(
            target: "nest_rs::schedule",
            provider = id.provider,
            method = id.method,
            error = %err,
            "scheduled job failed",
        );
    }
}
