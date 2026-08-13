//! Drive `Scheduler` end-to-end against a hand-built container. Metadata is
//! attached directly (`attach_meta` only needs a `'static` host type), so the
//! test needs neither `#[scheduled]` nor a module tree.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use nest_rs_core::{Container, Transport};
use nest_rs_schedule::nest_rs_worker::{JobSettlement, JobTransaction};
use nest_rs_schedule::{CronExpression, CronJobMeta, Scheduler, Trigger};
use nest_rs_worker::{self, JobContext};
use tokio_util::sync::CancellationToken;

static INTERVAL_HITS: AtomicU64 = AtomicU64::new(0);
static TIMEOUT_HITS: AtomicU64 = AtomicU64::new(0);
static CRON_HITS: AtomicU64 = AtomicU64::new(0);
static PANIC_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static SURVIVOR_HITS: AtomicU64 = AtomicU64::new(0);

type RunFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

fn tick_interval(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        INTERVAL_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

fn tick_timeout(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        TIMEOUT_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

fn tick_cron(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        CRON_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler_runs_interval_timeout_and_cron_jobs() {
    struct IntervalHost;
    struct TimeoutHost;
    struct CronHost;

    let container = Container::builder()
        .attach_meta::<IntervalHost, CronJobMeta>(CronJobMeta {
            provider: "IntervalHost",
            method: "interval",
            trigger: Trigger::Interval(Duration::from_millis(200)),
            run: tick_interval,
            transaction: JobTransaction::PerAttempt,
        })
        .attach_meta::<TimeoutHost, CronJobMeta>(CronJobMeta {
            provider: "TimeoutHost",
            method: "timeout",
            trigger: Trigger::Timeout(Duration::from_millis(300)),
            run: tick_timeout,
            transaction: JobTransaction::PerAttempt,
        })
        .attach_meta::<CronHost, CronJobMeta>(CronJobMeta {
            provider: "CronHost",
            method: "cron",
            trigger: Trigger::Cron {
                expr: CronExpression::EVERY_SECOND,
                tz: None,
            },
            run: tick_cron,
            transaction: JobTransaction::PerAttempt,
        })
        .build();

    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));

    // ~2.2s covers ~10 interval ticks, the one-shot at 300ms, and crosses a
    // whole-second boundary for the cron.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert!(
        INTERVAL_HITS.load(Ordering::SeqCst) >= 2,
        "interval job fires repeatedly",
    );
    assert_eq!(
        TIMEOUT_HITS.load(Ordering::SeqCst),
        1,
        "one-shot job fires exactly once",
    );
    assert!(
        CRON_HITS.load(Ordering::SeqCst) >= 1,
        "cron job fires at least once",
    );
}

fn tick_panic(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        PANIC_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        panic!("boom from a scheduled job");
    })
}

fn tick_survivor(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        SURVIVOR_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

/// B-SCHED: a panicking job must not silently and permanently stop its own
/// schedule, nor take a co-scheduled job's task down with it. Both jobs fire
/// repeatedly and `serve` returns `Ok` rather than aborting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_job_keeps_firing_and_does_not_stop_others() {
    struct PanicHost;
    struct SurvivorHost;

    let container = Container::builder()
        .attach_meta::<PanicHost, CronJobMeta>(CronJobMeta {
            provider: "PanicHost",
            method: "panics",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_panic,
            transaction: JobTransaction::PerAttempt,
        })
        .attach_meta::<SurvivorHost, CronJobMeta>(CronJobMeta {
            provider: "SurvivorHost",
            method: "survives",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_survivor,
            transaction: JobTransaction::PerAttempt,
        })
        .build();

    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(650)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins despite a panicking job")
        .expect("serve returns Ok despite a panicking job");

    assert!(
        PANIC_ATTEMPTS.load(Ordering::SeqCst) >= 2,
        "the panicking job is re-scheduled after each panic (attempts: {})",
        PANIC_ATTEMPTS.load(Ordering::SeqCst),
    );
    assert!(
        SURVIVOR_HITS.load(Ordering::SeqCst) >= 2,
        "the co-scheduled job keeps firing while its neighbour panics (hits: {})",
        SURVIVOR_HITS.load(Ordering::SeqCst),
    );
}

#[tokio::test]
async fn invalid_cron_expression_fails_configure() {
    struct BadHost;

    let container = Container::builder()
        .attach_meta::<BadHost, CronJobMeta>(CronJobMeta {
            provider: "BadHost",
            method: "broken",
            trigger: Trigger::Cron {
                expr: "not a cron expression",
                tz: None,
            },
            run: tick_cron,
            transaction: JobTransaction::PerAttempt,
        })
        .build();

    let err = Scheduler::new()
        .configure(&container)
        .await
        .expect_err("an invalid cron expression aborts configure");
    assert!(
        err.to_string().contains("BadHost::broken"),
        "the error names the offending job: {err}",
    );
}

// A bound `JobContext` wraps each tick — the seam a database module uses to
// install a pool executor. The stub here installs an ambient marker the job
// observes.
tokio::task_local! {
    static MARKER: u8;
}

static OBSERVED_MARKER: AtomicBool = AtomicBool::new(false);

struct MarkerContext;

impl JobContext for MarkerContext {
    fn scope<'a>(
        &'a self,
        _transaction: JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
        Box::pin(async move {
            MARKER.scope(7, inner).await;
            JobSettlement::Settled
        })
    }
}

fn tick_observe(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        if MARKER.try_with(|m| *m) == Ok(7) {
            OBSERVED_MARKER.store(true, Ordering::SeqCst);
        }
        Ok(())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jobs_run_inside_the_bound_job_context() {
    struct ObserveHost;

    let container = Container::builder()
        .provide_dyn::<dyn JobContext>(Arc::new(MarkerContext))
        .attach_meta::<ObserveHost, CronJobMeta>(CronJobMeta {
            provider: "ObserveHost",
            method: "observe",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_observe,
            transaction: JobTransaction::PerAttempt,
        })
        .build();

    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(350)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert!(
        OBSERVED_MARKER.load(Ordering::SeqCst),
        "the tick ran inside the bound JobContext, observing its ambient marker",
    );
}

// A context that cannot honour what the job did — the shape a failed commit
// takes. A schedule has no retry budget and no dead-letter, so the
// classification it carries changes nothing here; what must not happen is the
// attempt passing for a success.
struct UnsettleableContext(nest_rs_worker::Unhonoured);

impl JobContext for UnsettleableContext {
    fn scope<'a>(
        &'a self,
        _transaction: JobTransaction,
        inner: Pin<Box<dyn Future<Output = bool> + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = JobSettlement> + Send + 'a>> {
        Box::pin(async move {
            inner.await;
            JobSettlement::Unhonoured(self.0)
        })
    }
}

fn tick_succeed(_: &Container) -> RunFuture<'_> {
    Box::pin(async { Ok(()) })
}

/// The job body returns `Ok` and its writes never landed, so the schedule has
/// to say so — its only outcome being the event an operator reads. Single-thread
/// runtime on purpose: `LogCapture` is thread-local, and the scheduler's spawned
/// task shares this thread here.
#[tokio::test]
async fn a_tick_its_context_could_not_settle_is_reported_as_failed() {
    struct UnsettleableHost;

    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder()
        .provide_dyn::<dyn JobContext>(Arc::new(UnsettleableContext(
            nest_rs_worker::Unhonoured::deterministic(
                "the job's transaction could not be committed",
            ),
        )))
        .attach_meta::<UnsettleableHost, CronJobMeta>(CronJobMeta {
            provider: "UnsettleableHost",
            method: "tick",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_succeed,
            transaction: JobTransaction::PerAttempt,
        })
        .build();

    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(120)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    let event = logs
        .find("nest_rs::schedule", "scheduled job failed")
        .into_iter()
        .next()
        .expect("a tick whose transaction could not be settled is reported at error");
    assert_eq!(event.level, "error");
    assert_eq!(event.field("provider").as_deref(), Some("UnsettleableHost"));
    assert!(
        event
            .field("error")
            .expect("the failure names itself")
            .contains("could not be committed"),
        "and it carries the context's own sentence, not a message the schedule \
         invented: {event:?}",
    );
    assert_eq!(
        event.field("retryable").as_deref(),
        Some("false"),
        "and the classification the context reached — a schedule has no budget \
         to spend on it, so reporting it is the whole of what it owes, and \
         reporting nothing while saying otherwise is what an audit found: \
         {event:?}",
    );
}
