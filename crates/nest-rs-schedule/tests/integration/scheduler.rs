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
use nest_rs_testing::LogCapture;
use nest_rs_worker::{self, JobContext};
use tokio_util::sync::CancellationToken;

static INTERVAL_HITS: AtomicU64 = AtomicU64::new(0);
static TIMEOUT_HITS: AtomicU64 = AtomicU64::new(0);
static CRON_HITS: AtomicU64 = AtomicU64::new(0);
static PANIC_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static SURVIVOR_HITS: AtomicU64 = AtomicU64::new(0);
static NEVER_HITS: AtomicU64 = AtomicU64::new(0);

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

fn tick_panic_naming_itself(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        panic!("boom from {}", "a job that names its own failure");
    })
}

/// A contained panic's only trace is this field, so the field has to carry what
/// the job said. `a_panicking_job_keeps_firing_and_does_not_stop_others` asserts
/// the schedule survives and is blind to the sentence — which is how
/// `panic_message(&panic)` shipped, unsizing the `Box` itself into the trait
/// object and answering `<non-string panic payload>` to every operator query.
/// Single-thread runtime for the reason above: `LogCapture` is thread-local.
#[tokio::test]
async fn a_panicking_jobs_own_message_reaches_the_operator() {
    struct NamedPanicHost;

    let logs = nest_rs_testing::LogCapture::install();
    let container = Container::builder()
        .attach_meta::<NamedPanicHost, CronJobMeta>(CronJobMeta {
            provider: "NamedPanicHost",
            method: "panics",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_panic_naming_itself,
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
        .expect("serve task joins despite a panicking job")
        .expect("serve returns Ok despite a panicking job");

    let event = logs
        .find(
            "nest_rs::schedule",
            "scheduled job panicked; the schedule continues",
        )
        .into_iter()
        .next()
        .expect("a contained panic is reported at error");
    assert_eq!(event.level, "error");
    assert_eq!(event.field("provider").as_deref(), Some("NamedPanicHost"));
    assert_eq!(
        event.field("panic").as_deref(),
        Some("boom from a job that names its own failure"),
        "the field carries the payload's own sentence, not the placeholder a \
         borrowed box downcasts to: {event:?}",
    );

    // And the tick still files the family's line, saying what ran and how it
    // ended — a clock has no caller, so this is the only place a tick reports
    // itself at all.
    let ran = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_schedule::unit::TICK,
        )
        .into_iter()
        .next()
        .expect("every tick files one line, panic included");
    assert_eq!(
        ran.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "a panicking tick is not reported as a plain error: {ran:?}",
    );
    assert_eq!(ran.field("provider").as_deref(), Some("NamedPanicHost"));
    assert!(ran.field("duration_ms").is_some());
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

fn tick_never(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        NEVER_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

/// A schedule that is valid, parses, and will never come round again — a
/// seven-field croner pattern pinned to a year in the past is the plainest
/// form, and a February 30th or a `2024-02-29`-shaped one-off is the shape a
/// real app reaches it by.
///
/// Nothing else reports it. `configure` succeeds (the pattern is well-formed),
/// `serve` returns `Ok`, and the job's task parks on the cancel token exactly
/// like a job waiting for a real occurrence — so a schedule that will never fire
/// again is indistinguishable, from the outside, from one that has not fired
/// yet. This line is the difference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cron_with_no_future_occurrence_says_so_rather_than_waiting_forever() {
    struct NeverHost;

    // Global: the job loop runs on a spawned task, so a thread-local capture
    // installed here would never see the event it exists to read.
    let logs = LogCapture::install_global();

    let container = Container::builder()
        .attach_meta::<NeverHost, CronJobMeta>(CronJobMeta {
            provider: "NeverHost",
            method: "never",
            trigger: Trigger::Cron {
                expr: "0 0 0 1 1 ? 2020",
                tz: None,
            },
            run: tick_never,
            transaction: JobTransaction::PerAttempt,
        })
        // Empty and *present*: `configure` also walks the link-time
        // `ScheduledMethod` registry, and with no gate seeded it starts every
        // `#[scheduled]` compiled into this binary — four panicking ticks from
        // another module's fixtures in 300 ms. `expect_one` discriminates only
        // by target and message, so one more `#[cron]` in this binary would
        // turn that into a false failure of this test.
        .provide(nest_rs_core::ReachableProviders(Default::default()))
        .build();

    let mut scheduler = Scheduler::new();
    scheduler.configure(&container).await.expect(
        "a pattern that is well-formed configures — being in the past is not a parse error",
    );

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("a job that will never fire is not a serve failure");

    assert_eq!(
        NEVER_HITS.load(Ordering::SeqCst),
        0,
        "the job never ran, which is the fact that needed announcing",
    );

    let event = logs.expect_one(
        "nest_rs::schedule",
        "cron job has no future occurrence; it will not run again",
    );
    assert_eq!(event.level, "warn");
    // Provider *and* method: an app with several `#[cron]` methods on one host
    // learns nothing from the host's name alone.
    assert_eq!(event.field("provider").as_deref(), Some("NeverHost"));
    assert_eq!(event.field("method").as_deref(), Some("never"));
}
