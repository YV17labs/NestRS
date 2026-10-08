//! Drive `Scheduler` end-to-end against a hand-built container, with metadata
//! attached directly.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_core::{Container, Transport};
use nest_rs_schedule::nest_rs_worker::{JOB_TIMEOUT, JobSettlement, JobTransaction};
use nest_rs_schedule::{
    CronExpression, CronJobMeta, Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError,
    Replicas, RunFn, Scheduler, Trigger,
};
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

#[tokio::test(start_paused = true)]
async fn scheduler_runs_interval_timeout_and_cron_jobs() {
    struct IntervalHost;
    struct TimeoutHost;
    struct CronHost;

    let container = crate::hermetic()
        .attach_meta::<IntervalHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "IntervalHost",
            method: "interval",
            trigger: Trigger::Interval(Duration::from_millis(200)),
            run: tick_interval,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
        })
        .attach_meta::<TimeoutHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "TimeoutHost",
            method: "timeout",
            trigger: Trigger::Timeout(Duration::from_millis(300)),
            run: tick_timeout,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
        })
        .attach_meta::<CronHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "CronHost",
            method: "cron",
            trigger: Trigger::Cron {
                expr: CronExpression::EVERY_SECOND,
                tz: None,
            },
            run: tick_cron,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
        })
        .build();

    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));

    // Covers ~10 interval ticks, the one-shot at 300ms, and a whole-second
    // boundary for the cron.
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

#[tokio::test(start_paused = true)]
async fn a_panicking_job_keeps_firing_and_does_not_stop_others() {
    struct PanicHost;
    struct SurvivorHost;

    let container = crate::hermetic()
        .attach_meta::<PanicHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "PanicHost",
            method: "panics",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_panic,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
        })
        .attach_meta::<SurvivorHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "SurvivorHost",
            method: "survives",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_survivor,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
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

static STUCK_STARTED: AtomicBool = AtomicBool::new(false);
static STUCK_DROPPED: AtomicBool = AtomicBool::new(false);

/// Flags its own drop, so the test can tell a tick stopped where it waited from
/// one that returned.
struct FlagsDrop;

impl Drop for FlagsDrop {
    fn drop(&mut self) {
        STUCK_DROPPED.store(true, Ordering::SeqCst);
    }
}

fn tick_stuck(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        let _flag = FlagsDrop;
        STUCK_STARTED.store(true, Ordering::SeqCst);
        std::future::pending::<()>().await;
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn a_tick_still_running_at_the_shutdown_bound_is_stopped_and_files_cancelled() {
    struct StuckHost;

    let logs = LogCapture::install();
    let container = crate::hermetic()
        .attach_meta::<StuckHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "StuckHost",
            method: "never_returns",
            trigger: Trigger::Timeout(Duration::from_millis(10)),
            run: tick_stuck,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");

    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    let started = tokio::time::Instant::now() + Duration::from_secs(60);
    while !STUCK_STARTED.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < started,
            "the tick starts within a minute of its 10ms trigger"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let asked = tokio::time::Instant::now();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(60), serving)
        .await
        .expect("serve returns within its bound, not when the tick does")
        .expect("serve task joins")
        .expect("serve returns Ok");
    let took = asked.elapsed();

    assert!(
        took >= Scheduler::SHUTDOWN_TIMEOUT
            && took < Scheduler::SHUTDOWN_TIMEOUT + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT,
        "the tick was given the bound and stopped at it: {took:?}",
    );
    assert!(
        STUCK_DROPPED.load(Ordering::SeqCst),
        "the tick was dropped where it waited"
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_schedule::unit::TICK.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
    assert_eq!(line.field("method").as_deref(), Some("never_returns"));
    assert!(
        line.trace_id.is_some(),
        "filed in the tick's own trace: {line:#?}"
    );
    let span = logs.expect_span(
        nest_rs_schedule::TARGET,
        nest_rs_schedule::unit::TICK.name(),
    );
    assert_eq!(line.trace_id, span.field("trace_id"), "{line:#?}");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the span exports what the line files: {:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    let stopped = logs.expect_one(
        nest_rs_schedule::TARGET,
        "scheduled ticks still running at the shutdown bound are stopped; each is dropped where \
         it waits and files its line cancelled",
    );
    assert_eq!(stopped.level, "warn");
    assert_eq!(
        stopped.field("jobs").as_deref(),
        Some("StuckHost::never_returns")
    );
    assert_eq!(stopped.field("running").as_deref(), Some("1"));
    logs.expect_none(
        nest_rs_schedule::TARGET,
        "stopped ticks did not unwind within their bound; they run on through the shutdown hooks",
    );
}

#[tokio::test]
async fn invalid_cron_expression_fails_configure() {
    struct BadHost;

    let container = crate::hermetic()
        .attach_meta::<BadHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "BadHost",
            method: "broken",
            trigger: Trigger::Cron {
                expr: "not a cron expression",
                tz: None,
            },
            run: tick_cron,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
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

#[tokio::test]
async fn a_cron_jobs_boot_line_names_its_zone_and_the_database_release_it_read() {
    struct ZonedHost;
    struct UtcHost;

    let cron = |provider, tz| CronJobMeta {
        origin: module_path!(),
        provider,
        method: "report",
        trigger: Trigger::Cron {
            expr: "0 0 9 * * *",
            tz,
        },
        run: tick_cron,
        timeout: JOB_TIMEOUT,
        transaction: JobTransaction::PerAttempt,
        replicas: Replicas::Each,
        key: None,
    };
    let container = crate::hermetic()
        .attach_meta::<ZonedHost, CronJobMeta>(cron("ZonedHost", Some("america/vancouver")))
        .attach_meta::<UtcHost, CronJobMeta>(cron("UtcHost", None))
        .build();
    let logs = nest_rs_testing::LogCapture::install();

    Scheduler::new()
        .configure(&container)
        .await
        .expect("both jobs configure");

    let line = |provider: &str| {
        logs.find(nest_rs_schedule::TARGET, "scheduled job (cron)")
            .into_iter()
            .find(|line| line.field("provider").as_deref() == Some(provider))
            .unwrap_or_else(|| panic!("{provider} files a boot line: {:#?}", logs.events()))
    };
    let zoned = line("ZonedHost");
    assert_eq!(
        zoned.field("timezone").as_deref(),
        Some("America/Vancouver"),
        "the zone as the database spells it: {zoned:?}"
    );
    assert_eq!(zoned.field("tzdb").as_deref(), jiff_tzdb::VERSION);
    let utc = line("UtcHost");
    assert_eq!(utc.field("timezone").as_deref(), Some("UTC"));
    assert_eq!(utc.field("tzdb"), None, "UTC is read from no database");
}

// A `JobContext` stub installing an ambient marker the job observes.
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

#[tokio::test(start_paused = true)]
async fn jobs_run_inside_the_bound_job_context() {
    struct ObserveHost;

    let container = crate::hermetic()
        .provide_dyn::<dyn JobContext>(Arc::new(MarkerContext))
        .attach_meta::<ObserveHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "ObserveHost",
            method: "observe",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_observe,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
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
// takes.
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

/// A contained panic's only trace is this field, so it carries what the job
/// said. Single-thread runtime: `LogCapture` is thread-local.
#[tokio::test(start_paused = true)]
async fn a_panicking_jobs_own_message_reaches_the_operator() {
    struct NamedPanicHost;

    let logs = nest_rs_testing::LogCapture::install();
    let container = crate::hermetic()
        .attach_meta::<NamedPanicHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "NamedPanicHost",
            method: "panics",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_panic_naming_itself,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
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
        event.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("boom from a job that names its own failure"),
        "the field carries the payload's own sentence, not the placeholder a \
         borrowed box downcasts to: {event:?}",
    );

    let ran = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_schedule::unit::TICK.name(),
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

fn tick_wrapped_failure(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        Err(
            anyhow::Error::new(std::io::Error::other("the store refused the write"))
                .context("syncing orgs"),
        )
    })
}

#[tokio::test(start_paused = true)]
async fn a_failed_tick_names_every_cause_beneath_its_error() {
    struct WrappedFailureHost;

    let logs = nest_rs_testing::LogCapture::install();
    let container = crate::hermetic()
        .attach_meta::<WrappedFailureHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "WrappedFailureHost",
            method: "tick",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_wrapped_failure,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::Each,
            key: None,
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
        .find(|event| event.field("provider").as_deref() == Some("WrappedFailureHost"))
        .expect("a failed tick is reported at error");
    assert_eq!(
        event.field("error").as_deref(),
        Some("syncing orgs: the store refused the write"),
        "the wrapper alone names nothing an operator acts on: {event:?}",
    );
}

/// A reply a tick could not decode, behind a wrapper that hides it from
/// `source()` — thiserror's `transparent` — and a context line over that.
fn tick_decode_failure(_: &Container) -> RunFuture<'_> {
    use nest_rs_core::serde::Deserialize;
    use nest_rs_core::serde::de::IntoDeserializer;
    use nest_rs_core::serde::de::value::Error as ValueError;

    #[derive(Debug, thiserror::Error)]
    #[error(transparent)]
    struct Reply(ValueError);

    Box::pin(async {
        let refused = u64::deserialize(IntoDeserializer::<ValueError>::into_deserializer(
            "sk_live_51HsecretTOKEN",
        ))
        .expect_err("a secret is not a number");
        Err(anyhow::Error::new(Reply(refused)).context("reading the upstream reply"))
    })
}

#[tokio::test(start_paused = true)]
async fn a_failed_tick_says_a_decode_failure_without_its_value() {
    struct DecodingHost;

    let logs = nest_rs_testing::LogCapture::install();
    let container = crate::hermetic()
        .attach_meta::<DecodingHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "DecodingHost",
            method: "tick",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_decode_failure,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::Each,
            key: None,
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
        .find(|event| event.field("provider").as_deref() == Some("DecodingHost"))
        .expect("a failed tick is reported at error");
    assert_eq!(
        event.field("error").as_deref(),
        Some("reading the upstream reply: invalid type: a string, expected u64"),
    );
    assert!(
        logs.events()
            .iter()
            .all(|event| !event.fields.values().any(|value| value.contains("sk_live"))),
        "no line quotes the value",
    );
}

/// Single-thread runtime: `LogCapture` is thread-local, and the scheduler's
/// spawned task shares this thread here.
#[tokio::test(start_paused = true)]
async fn a_tick_its_context_could_not_settle_is_reported_as_failed() {
    struct UnsettleableHost;

    let logs = nest_rs_testing::LogCapture::install();
    let container = crate::hermetic()
        .provide_dyn::<dyn JobContext>(Arc::new(UnsettleableContext(
            nest_rs_worker::Unhonoured::deterministic(
                "the job's transaction could not be committed",
            ),
        )))
        .attach_meta::<UnsettleableHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "UnsettleableHost",
            method: "tick",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_succeed,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
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

static HUNG_STARTS: AtomicU64 = AtomicU64::new(0);

fn tick_hung(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        HUNG_STARTS.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
        Ok(())
    })
}

/// A replica runs one occurrence at a time, so a tick that never returns would
/// skip every later one.
#[tokio::test(start_paused = true)]
async fn a_tick_past_its_timeout_is_cut_reported_and_the_schedule_goes_on() {
    struct HungHost;

    let logs = nest_rs_testing::LogCapture::install();
    let container = crate::hermetic()
        .attach_meta::<HungHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "HungHost",
            method: "tick",
            trigger: Trigger::Interval(Duration::from_secs(2)),
            run: tick_hung,
            timeout: Duration::from_secs(1),
            transaction: JobTransaction::Pool,
            replicas: Replicas::Each,
            key: None,
        })
        .build();

    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("scheduler configures against the container");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(5_500)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert!(
        HUNG_STARTS.load(Ordering::SeqCst) >= 2,
        "the occurrence after a cut one fires"
    );
    let event = logs
        .find("nest_rs::schedule", "scheduled job failed")
        .into_iter()
        .find(|event| event.field("provider").as_deref() == Some("HungHost"))
        .expect("a tick cut at its timeout is reported at error");
    assert!(
        event
            .field("error")
            .is_some_and(|error| error.contains("1s timeout") && error.contains("`timeout`")),
        "the line names the deadline and the key setting it: {event:?}",
    );
    assert_eq!(
        event.field("retryable").as_deref(),
        Some("true"),
        "a cut says nothing about what the next occurrence reads: {event:?}",
    );
}

fn tick_never(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        NEVER_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

/// A schedule that is valid and will never come round again: from the outside,
/// indistinguishable from one that has not fired yet but for this line.
#[tokio::test(start_paused = true)]
async fn a_cron_with_no_future_occurrence_says_so_rather_than_waiting_forever() {
    struct NeverHost;

    // Global: the job loop runs on a spawned task.
    let logs = LogCapture::install_global();

    let container = crate::hermetic()
        .attach_meta::<NeverHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "NeverHost",
            method: "never",
            trigger: Trigger::Cron {
                expr: "0 0 0 1 1 ? 2020",
                tz: None,
            },
            run: tick_never,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
        })
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
    assert_eq!(event.field("provider").as_deref(), Some("NeverHost"));
    assert_eq!(event.field("method").as_deref(), Some("never"));
}

/// A lock every replica in a test reaches, standing in for the store a
/// deployment shares: it records each claim and grants an occurrence to its
/// first claimant only.
#[derive(Default)]
struct SharedLock {
    claims: std::sync::Mutex<Vec<(String, Duration)>>,
    granted: std::sync::Mutex<std::collections::HashSet<String>>,
}

#[async_trait::async_trait]
impl OccurrenceLock for SharedLock {
    async fn claim(&self, occurrence: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError> {
        self.claims
            .lock()
            .expect("lock")
            .push((occurrence.token.clone(), occurrence.hold));
        if !self
            .granted
            .lock()
            .expect("lock")
            .insert(occurrence.token.clone())
        {
            return Ok(OccurrenceClaim::ClaimedElsewhere);
        }
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, token: &str) -> Result<bool, OccurrenceLockError> {
        Ok(self.granted.lock().expect("lock").contains(token))
    }
}

/// The crate this suite is compiled as.
fn crate_name() -> &'static str {
    module_path!()
        .split_once("::")
        .map_or(module_path!(), |(krate, _)| krate)
}

/// The instant an occurrence key names, in milliseconds since the epoch.
fn instant_of(occurrence: &str) -> u64 {
    occurrence
        .rsplit(':')
        .next()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or_else(|| panic!("an occurrence ends with its instant: {occurrence}"))
}

static ONCE_HITS: AtomicU64 = AtomicU64::new(0);

fn tick_once(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        ONCE_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn two_replicas_sharing_a_lock_fire_each_occurrence_once() {
    struct OnceHost;
    const PERIOD: Duration = Duration::from_millis(200);
    // Read by the assertion below rather than retyped.
    const PROVIDER: &str = "OnceHost";
    const METHOD: &str = "once";

    let lock = Arc::new(SharedLock::default());
    let cancel = CancellationToken::new();
    let mut serving = Vec::new();
    for _ in 0..2 {
        let shared: Arc<dyn OccurrenceLock> = lock.clone();
        let container = crate::hermetic()
            .provide_dyn::<dyn OccurrenceLock>(shared)
            .attach_meta::<OnceHost, CronJobMeta>(CronJobMeta {
                origin: module_path!(),
                provider: PROVIDER,
                method: METHOD,
                trigger: Trigger::Interval(PERIOD),
                run: tick_once,
                timeout: JOB_TIMEOUT,
                transaction: JobTransaction::Pool,
                replicas: Replicas::One,
                key: None,
            })
            .build();
        let mut scheduler = Scheduler::new();
        scheduler
            .configure(&container)
            .await
            .expect("a replica configures with a lock bound");
        serving.push(tokio::spawn(Box::new(scheduler).serve(cancel.clone())));
        // The second replica boots mid-period, the way a scale-up does.
        tokio::time::sleep(Duration::from_millis(70)).await;
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    cancel.cancel();
    for replica in serving {
        replica
            .await
            .expect("serve task joins")
            .expect("serve returns Ok");
    }

    let claims = lock.claims.lock().expect("lock").clone();
    let granted = lock.granted.lock().expect("lock").len();
    let fired = ONCE_HITS.load(Ordering::SeqCst) as usize;
    assert!(granted >= 3, "occurrences were claimed: {claims:?}");
    assert_eq!(
        fired, granted,
        "each occurrence fires once, on the replica that claimed it",
    );
    assert!(
        claims.len() > granted,
        "both replicas reached the same occurrences, so some claim was lost: {claims:?}",
    );
    for (occurrence, hold) in &claims {
        assert_eq!(
            instant_of(occurrence) % PERIOD.as_millis() as u64,
            0,
            "a replica booted mid-period still ticks on the epoch: {occurrence}",
        );
        assert!(
            *hold >= Duration::from_secs(60),
            "a claim outlasts clock skew: {hold:?}"
        );
        assert!(
            occurrence.starts_with(&format!("{}:{PROVIDER}:{METHOD}:", crate_name())),
            "an occurrence is the job's crate, provider and method, a level each: {occurrence}",
        );
    }
}

static CRON_ONCE_HITS: AtomicU64 = AtomicU64::new(0);

static OVERLAP_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static OVERLAP_MOST: AtomicU64 = AtomicU64::new(0);
static OVERLAP_RUNS: AtomicU64 = AtomicU64::new(0);

/// A run three and a half periods long, counting how many are in flight at once.
fn tick_overlapping(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        let in_flight = OVERLAP_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        OVERLAP_MOST.fetch_max(in_flight, Ordering::SeqCst);
        OVERLAP_RUNS.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(350)).await;
        OVERLAP_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    })
}

/// Once per occurrence, and nothing about runs: a run outlasting its period
/// overlaps the next on another replica. A run lease fails this test
/// (`.claude/decisions/schedule-run-lease.md`).
#[tokio::test(start_paused = true)]
async fn a_run_outlasting_its_period_leaves_the_next_occurrences_to_its_peers() {
    struct OverlapHost;
    const PERIOD: Duration = Duration::from_millis(100);

    let logs = LogCapture::install_global();
    let lock = Arc::new(SharedLock::default());
    let cancel = CancellationToken::new();
    let mut serving = Vec::new();
    for _ in 0..3 {
        let shared: Arc<dyn OccurrenceLock> = lock.clone();
        let container = crate::hermetic()
            .provide_dyn::<dyn OccurrenceLock>(shared)
            .attach_meta::<OverlapHost, CronJobMeta>(CronJobMeta {
                origin: module_path!(),
                provider: "OverlapHost",
                method: "overlap",
                trigger: Trigger::Interval(PERIOD),
                run: tick_overlapping,
                timeout: JOB_TIMEOUT,
                transaction: JobTransaction::Pool,
                replicas: Replicas::One,
                key: None,
            })
            .build();
        let mut scheduler = Scheduler::new();
        scheduler
            .configure(&container)
            .await
            .expect("a replica configures with a lock bound");
        serving.push(tokio::spawn(Box::new(scheduler).serve(cancel.clone())));
    }
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    cancel.cancel();
    for replica in serving {
        replica
            .await
            .expect("serve task joins")
            .expect("serve returns Ok");
    }

    let granted = lock.granted.lock().expect("lock").len() as u64;
    assert!(granted >= 4, "occurrences were claimed within 1.6 s");
    assert_eq!(
        OVERLAP_RUNS.load(Ordering::SeqCst),
        granted,
        "each occurrence fired once, on the replica that claimed it"
    );
    assert!(
        OVERLAP_MOST.load(Ordering::SeqCst) >= 2,
        "a peer fired the next occurrence while a run went on, so two ran at once"
    );
    let claimed_elsewhere: u64 = logs
        .find(
            nest_rs_schedule::TARGET,
            "occurrences overrun on this replica, each claimed by another",
        )
        .into_iter()
        .filter(|event| event.field("provider").as_deref() == Some("OverlapHost"))
        .chain(
            logs.find(
                nest_rs_schedule::TARGET,
                "occurrences skipped: they fell due while the previous one was claimed or run",
            )
            .into_iter()
            .filter(|event| event.field("provider").as_deref() == Some("OverlapHost")),
        )
        .filter_map(|event| {
            event
                .field("claimed_elsewhere")
                .or_else(|| event.field("overrun"))
                .and_then(|count| count.parse::<u64>().ok())
        })
        .sum();
    assert!(
        claimed_elsewhere >= 1,
        "the replica whose run overran its peers' occurrences heard they were claimed: {:#?}",
        logs.events()
    );
}

fn tick_cron_once(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        CRON_ONCE_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn a_cron_firing_on_one_replica_claims_the_instant_its_expression_names() {
    struct CronOnceHost;

    let lock = Arc::new(SharedLock::default());
    let shared: Arc<dyn OccurrenceLock> = lock.clone();
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(shared)
        .attach_meta::<CronOnceHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "CronOnceHost",
            method: "each_second",
            trigger: Trigger::Cron {
                expr: CronExpression::EVERY_SECOND,
                tz: None,
            },
            run: tick_cron_once,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(2300)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    let claims = lock.claims.lock().expect("lock").clone();
    assert!(
        claims.len() >= 2,
        "the cron claimed its occurrences: {claims:?}"
    );
    assert_eq!(CRON_ONCE_HITS.load(Ordering::SeqCst) as usize, claims.len());
    for (occurrence, hold) in &claims {
        assert_eq!(instant_of(occurrence) % 1_000, 0, "{occurrence}");
        assert!(*hold >= Duration::from_secs(60), "{hold:?}");
    }
}

#[tokio::test]
async fn a_job_firing_on_one_replica_fails_the_boot_without_a_lock() {
    struct LonelyHost;

    let container = crate::hermetic()
        .attach_meta::<LonelyHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "LonelyHost",
            method: "once",
            trigger: Trigger::Interval(Duration::from_secs(5)),
            run: tick_once,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();

    let err = Scheduler::new()
        .configure(&container)
        .await
        .expect_err("no lock is bound to claim the job's occurrences");
    let msg = err.to_string();
    assert!(msg.contains("LonelyHost::once"), "names the job: {msg}");
    assert!(
        msg.contains(nest_rs_schedule::BACKEND_REMEDY),
        "and the one remedy every lock binding shares: {msg}",
    );
}

#[tokio::test]
async fn a_one_shot_firing_on_one_replica_fails_the_boot() {
    struct OneShotHost;

    let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<OneShotHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "OneShotHost",
            method: "warmup",
            trigger: Trigger::Timeout(Duration::from_millis(10)),
            run: tick_once,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();

    let err = Scheduler::new()
        .configure(&container)
        .await
        .expect_err("a one-shot declaring one replica is refused");
    assert!(err.to_string().contains("OneShotHost::warmup"), "{err}");
}

/// A zero interval panics the timer; a job registered by hand is refused at
/// boot.
#[tokio::test]
async fn a_zero_interval_fails_the_boot() {
    struct ZeroHost;

    let container = crate::hermetic()
        .attach_meta::<ZeroHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "ZeroHost",
            method: "spin",
            trigger: Trigger::Interval(Duration::ZERO),
            run: tick_once,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::Each,
            key: None,
        })
        .build();

    let err = Scheduler::new()
        .configure(&container)
        .await
        .expect_err("a zero interval is refused");
    assert!(err.to_string().contains("ZeroHost::spin"), "{err}");
}

#[tokio::test]
async fn a_sub_millisecond_interval_fails_the_boot_whatever_its_replicas() {
    struct SubMillisecondHost;

    for replicas in [Replicas::Each, Replicas::One] {
        let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
        let container = crate::hermetic()
            .provide_dyn::<dyn OccurrenceLock>(lock)
            .attach_meta::<SubMillisecondHost, CronJobMeta>(CronJobMeta {
                origin: module_path!(),
                provider: "SubMillisecondHost",
                method: "spin",
                trigger: Trigger::Interval(Duration::from_micros(500)),
                run: tick_once,
                timeout: JOB_TIMEOUT,
                transaction: JobTransaction::Pool,
                replicas,
                key: None,
            })
            .build();

        let err = Scheduler::new()
            .configure(&container)
            .await
            .expect_err("an interval under a millisecond is refused")
            .to_string();
        assert!(
            err.contains("SubMillisecondHost::spin")
                && err.contains("an interval is at least one millisecond"),
            "{replicas:?}: {err}"
        );
    }
}

/// A failure with a cause beneath it, so the assertion sees whether the
/// scheduler renders the chain.
fn unreachable_lock() -> OccurrenceLockError {
    OccurrenceLockError::new(
        anyhow::Error::new(std::io::Error::other("connection refused by redis:6379"))
            .context("the lock store is unreachable"),
    )
}

/// A lock that cannot answer — the store unreachable.
struct FailingLock;

#[async_trait::async_trait]
impl OccurrenceLock for FailingLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        Err(unreachable_lock())
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        Err(unreachable_lock())
    }
}

static UNCLAIMED_HITS: AtomicU64 = AtomicU64::new(0);

fn tick_unclaimed(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        UNCLAIMED_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

/// Single-thread runtime: `LogCapture` is thread-local.
#[tokio::test(start_paused = true)]
async fn an_occurrence_whose_lock_fails_is_skipped_and_says_so() {
    struct UnclaimedHost;

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(FailingLock);
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<UnclaimedHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "UnclaimedHost",
            method: "once",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_unclaimed,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(180)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert_eq!(
        UNCLAIMED_HITS.load(Ordering::SeqCst),
        0,
        "an occurrence nobody claimed never fires",
    );
    let skipped = logs.find(
        "nest_rs::schedule",
        "occurrence skipped: its lock could not be claimed",
    );
    let event = skipped
        .first()
        .unwrap_or_else(|| panic!("the skip is reported: {:#?}", logs.events()));
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("provider").as_deref(), Some("UnclaimedHost"));
    assert_eq!(event.field("method").as_deref(), Some("once"));
    let occurrence: u64 = event
        .field("occurrence")
        .and_then(|ms| ms.parse().ok())
        .unwrap_or_else(|| panic!("the event names the occurrence: {event:?}"));
    assert_eq!(occurrence % 50, 0, "{occurrence}");
    assert_eq!(
        event.field("error").as_deref(),
        Some("the lock store is unreachable: connection refused by redis:6379"),
        "the event carries the failure and the cause beneath it: {event:?}",
    );

    // Filtered by provider: with no reachable set seeded, every `#[scheduled]`
    // method linked into the binary files a boot line too.
    let boot = logs
        .find("nest_rs::schedule", "scheduled job (interval)")
        .into_iter()
        .find(|line| line.field("provider").as_deref() == Some("UnclaimedHost"))
        .unwrap_or_else(|| panic!("the job files a boot line: {:#?}", logs.events()));
    assert_eq!(
        boot.field("replicas").as_deref(),
        Some("one"),
        "the boot line says which jobs fire once: {boot:?}",
    );
}

/// A lock whose first claim takes longer than the job's period — a store
/// answering slowly, or a connect budget being run out — and grants it; every
/// later claim is a peer's, so the count of fires stays one, and no peer fired
/// the occurrences the slow claim overran.
#[derive(Default)]
struct SlowLock {
    claimed: AtomicBool,
}

#[async_trait::async_trait]
impl OccurrenceLock for SlowLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        if self.claimed.swap(true, Ordering::SeqCst) {
            return Ok(OccurrenceClaim::ClaimedElsewhere);
        }
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        Ok(false)
    }
}

static SLOWLY_CLAIMED_HITS: AtomicU64 = AtomicU64::new(0);

fn tick_slowly_claimed(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        SLOWLY_CLAIMED_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn occurrences_overrun_by_a_slow_claim_are_skipped_and_counted_aloud() {
    struct OverrunHost;
    const PERIOD: Duration = Duration::from_millis(200);

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(SlowLock::default());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<OverrunHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "OverrunHost",
            method: "sweep",
            trigger: Trigger::Interval(PERIOD),
            run: tick_slowly_claimed,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert_eq!(
        SLOWLY_CLAIMED_HITS.load(Ordering::SeqCst),
        1,
        "one occurrence claimed and fired: the latest overrun one is claimed late, by a peer \
         here, and the ones before it are skipped",
    );
    let skipped = logs.find(
        "nest_rs::schedule",
        "occurrences skipped: they fell due while the previous one was claimed or run",
    );
    let event = skipped
        .iter()
        .find(|event| event.field("provider").as_deref() == Some("OverrunHost"))
        .unwrap_or_else(|| panic!("the skipped stretch is reported: {:#?}", logs.events()));
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("method").as_deref(), Some("sweep"));
    let count: u64 = event
        .field("skipped")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("the event counts them: {event:?}"));
    assert!(
        (4..=6).contains(&count),
        "a claim of 1.1 s over a period of 200 ms overruns about five: {event:?}",
    );
    assert!(
        event.field("occurrence").is_some() && event.field("overrun_ms").is_some(),
        "from which occurrence, and by how long: {event:?}",
    );
    assert_eq!(
        event.field("claimed_elsewhere").as_deref(),
        Some("0"),
        "the lock was asked, and no peer had fired them: {event:?}",
    );
    let tick = logs
        .find("nest_rs::operation", "schedule.tick")
        .into_iter()
        .find(|line| line.field("provider").as_deref() == Some("OverrunHost"))
        .unwrap_or_else(|| panic!("the fire files its line: {:#?}", logs.events()));
    assert!(
        tick.field("occurrence")
            .is_some_and(|instant| instant.parse::<u64>().is_ok_and(|ms| ms % 200 == 0)),
        "a job firing once carries the occurrence it claimed on its line: {tick:?}",
    );
}

fn tick_noop(_: &Container) -> RunFuture<'_> {
    Box::pin(async { Ok(()) })
}

#[tokio::test]
async fn two_jobs_sharing_one_name_fail_the_boot_naming_both() {
    struct FirstTasks;
    struct SecondTasks;

    let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
    let meta = || CronJobMeta {
        origin: module_path!(),
        provider: "Tasks",
        method: "sweep",
        trigger: Trigger::Interval(Duration::from_secs(1)),
        run: tick_noop,
        timeout: JOB_TIMEOUT,
        transaction: JobTransaction::Pool,
        replicas: Replicas::One,
        key: None,
    };
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<FirstTasks, CronJobMeta>(meta())
        .attach_meta::<SecondTasks, CronJobMeta>(meta())
        .build();
    let refusal = Scheduler::new()
        .configure(&container)
        .await
        .expect_err("one name, two jobs")
        .to_string();
    assert!(
        refusal.contains("`Tasks::sweep`") && refusal.contains("share one name"),
        "{refusal}"
    );
    assert!(
        refusal.contains(&format!("(declared twice in {})", module_path!())),
        "one site, named once: {refusal}"
    );
}

/// What the boot answers a job attached by hand that pins `key`, firing as
/// `replicas` says: `Ok` when it configures, the refusal otherwise.
async fn boot_pinning(key: &'static str, replicas: Replicas) -> Result<(), String> {
    struct PinnedTasks;
    let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<PinnedTasks, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "PinnedTasks",
            method: "sweep",
            trigger: Trigger::Interval(Duration::from_secs(1)),
            run: tick_noop,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas,
            key: Some(key),
        })
        .build();
    Scheduler::new()
        .configure(&container)
        .await
        .map_err(|refusal| refusal.to_string())
}

/// The boot's copy of the decorator's `key` rule, run over the decorator's
/// corpus so the two cannot disagree.
#[tokio::test]
async fn the_decorators_copy_of_the_key_rule_agrees_with_the_boots() {
    const CORPUS: [&str; 13] = [
        "billing::InvoiceTasks::close_day",
        "nightly_close",
        "billing.nightly-close",
        "café::Tâches::purger",
        "",
        "billing::",
        "::billing",
        "billing:::close",
        "billing:close",
        "billing::close day",
        "billing::\tclose",
        "billing::\u{7}",
        "a::b::c::d::e",
    ];
    for key in CORPUS {
        let boot = boot_pinning(key, Replicas::One).await;
        assert_eq!(
            boot.is_ok(),
            nest_rs_codegen::is_valid_job_key(key),
            "the two copies disagree on {key:?}: {boot:?}"
        );
        if let Err(refusal) = boot {
            let compile = nest_rs_codegen::invalid_job_key("every", key);
            let fact = compile
                .strip_prefix("#[every] `key`: ")
                .expect("the decorator's refusal opens with its site");
            assert!(
                refusal.contains(fact),
                "one fact at both sites: {refusal} / {compile}"
            );
        }
    }
}

#[tokio::test]
async fn a_key_on_a_job_firing_on_every_replica_fails_the_boot() {
    let refusal = boot_pinning("billing::PinnedTasks::sweep", Replicas::Each)
        .await
        .expect_err("a key nothing claims under");
    let compile = nest_rs_codegen::key_without_replicas_one(nest_rs_codegen::JobDecorator::Every);
    let fact = compile
        .strip_prefix("#[every] `key`: ")
        .expect("the decorator's refusal opens with its site");
    assert!(
        refusal.contains("`PinnedTasks::sweep`") && refusal.contains(fact),
        "{refusal}"
    );
}

#[tokio::test]
async fn two_jobs_firing_once_under_one_identity_fail_the_boot_naming_both() {
    struct InvoiceTasks;
    struct LedgerTasks;

    let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
    let derived = format!("{}::InvoiceTasks::close_day", crate_name());
    let pinned: &'static str = Box::leak(derived.clone().into_boxed_str());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<InvoiceTasks, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "InvoiceTasks",
            method: "close_day",
            trigger: Trigger::Interval(Duration::from_secs(1)),
            run: tick_noop,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .attach_meta::<LedgerTasks, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "LedgerTasks",
            method: "close",
            trigger: Trigger::Interval(Duration::from_secs(1)),
            run: tick_noop,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: Some(pinned),
        })
        .build();
    let refusal = Scheduler::new()
        .configure(&container)
        .await
        .expect_err("one identity, two jobs")
        .to_string();
    assert!(
        refusal.contains(&format!("`{derived}` by `InvoiceTasks::close_day`"))
            && refusal.contains("`LedgerTasks::close`"),
        "{refusal}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_job_firing_once_names_its_identity_at_boot() {
    struct OnceTasks;
    struct PinnedOnceTasks;
    struct EachTasks;

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
    let meta = |provider, replicas, key| CronJobMeta {
        origin: module_path!(),
        provider,
        method: "sweep",
        trigger: Trigger::Interval(Duration::from_secs(1)),
        run: tick_noop,
        timeout: JOB_TIMEOUT,
        transaction: JobTransaction::Pool,
        replicas,
        key,
    };
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<OnceTasks, CronJobMeta>(meta("OnceTasks", Replicas::One, None))
        .attach_meta::<PinnedOnceTasks, CronJobMeta>(meta(
            "PinnedOnceTasks",
            Replicas::One,
            Some("billing::close"),
        ))
        .attach_meta::<EachTasks, CronJobMeta>(meta("EachTasks", Replicas::Each, None))
        .build();
    Scheduler::new()
        .configure(&container)
        .await
        .expect("three jobs, three identities");

    let booted = |provider: &str| {
        logs.find(nest_rs_schedule::TARGET, "scheduled job (interval)")
            .into_iter()
            .find(|line| line.field("provider").as_deref() == Some(provider))
            .unwrap_or_else(|| panic!("{provider} files its boot line"))
            .field("key")
    };
    assert_eq!(
        booted("OnceTasks"),
        Some(format!("{}::OnceTasks::sweep", crate_name()))
    );
    assert_eq!(booted("PinnedOnceTasks").as_deref(), Some("billing::close"));
    assert_eq!(booted("EachTasks"), None);
}

/// A lock whose first claim takes longer than the job's period and grants it,
/// and which answers that every occurrence after it was claimed: a peer fired
/// them while this replica was blocked.
#[derive(Default)]
struct PeerFiredLock {
    claimed: AtomicBool,
}

#[async_trait::async_trait]
impl OccurrenceLock for PeerFiredLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        if self.claimed.swap(true, Ordering::SeqCst) {
            return Ok(OccurrenceClaim::ClaimedElsewhere);
        }
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        Ok(true)
    }
}

#[tokio::test(start_paused = true)]
async fn occurrences_a_peer_claimed_while_this_replica_overran_are_not_reported_skipped() {
    struct PeerFiredHost;
    const PERIOD: Duration = Duration::from_millis(200);

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(PeerFiredLock::default());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<PeerFiredHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "PeerFiredHost",
            method: "sweep",
            trigger: Trigger::Interval(PERIOD),
            run: tick_noop,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    let for_host = |message: &str| {
        logs.find("nest_rs::schedule", message)
            .into_iter()
            .filter(|event| event.field("provider").as_deref() == Some("PeerFiredHost"))
            .collect::<Vec<_>>()
    };
    assert!(
        for_host("occurrences skipped: they fell due while the previous one was claimed or run")
            .is_empty(),
        "a peer fired them: {:#?}",
        logs.events(),
    );
    let overrun = for_host("occurrences overrun on this replica, each claimed by another");
    let event = overrun
        .first()
        .unwrap_or_else(|| panic!("the overrun is still said, at debug: {:#?}", logs.events()));
    assert_eq!(event.level, "debug");
    assert!(
        event
            .field("overrun")
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| (4..=8).contains(&n)),
        "a claim of 1.1 s over a period of 200 ms overruns about five: {event:?}",
    );
}

fn tick_a_little_longer_than_its_period(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        tokio::time::sleep(Duration::from_millis(350)).await;
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn an_occurrence_a_run_overran_by_less_than_a_period_fires_late_rather_than_skipped() {
    struct BackToBackHost;
    const PERIOD_MS: u64 = 300;

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<BackToBackHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "BackToBackHost",
            method: "crunch",
            trigger: Trigger::Interval(Duration::from_millis(PERIOD_MS)),
            run: tick_a_little_longer_than_its_period,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(1_800)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    let fired: Vec<u64> = logs
        .find("nest_rs::operation", "schedule.tick")
        .into_iter()
        .filter(|line| line.field("provider").as_deref() == Some("BackToBackHost"))
        .map(|line| {
            line.field("occurrence")
                .and_then(|instant| instant.parse().ok())
                .unwrap_or_else(|| panic!("a job firing once carries its occurrence: {line:?}"))
        })
        .collect();
    assert!(
        fired.len() >= 3,
        "a 350 ms run every 300 ms runs back to back: {fired:?}"
    );
    assert_eq!(
        [fired[1] - fired[0], fired[2] - fired[1]],
        [PERIOD_MS, PERIOD_MS],
        "each run claims the occurrence that fell due while the one before it ran: {fired:?}",
    );
}

/// When each fire of the two stalled jobs below started, on the wall clock.
static STALLED_INTERVAL_FIRES: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
static STALLED_CRON_FIRES: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

/// The wall clock as the scheduler reads it: on paused time, the system clock
/// moved by however far the paused clock advanced past the monotonic one.
fn wall_ms() -> u64 {
    let wall = SystemTime::now();
    let paused = tokio::time::Instant::now().into_std();
    let monotonic = std::time::Instant::now();
    let wall = match paused.checked_duration_since(monotonic) {
        Some(ahead) => wall + ahead,
        None => wall - monotonic.duration_since(paused),
    };
    u64::try_from(
        wall.duration_since(UNIX_EPOCH)
            .expect("a clock after the epoch")
            .as_millis(),
    )
    .expect("in range")
}

fn tick_stalled_interval(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        STALLED_INTERVAL_FIRES.lock().expect("lock").push(wall_ms());
        Ok(())
    })
}

fn tick_stalled_cron(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        STALLED_CRON_FIRES.lock().expect("lock").push(wall_ms());
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn a_replica_stalled_past_several_occurrences_fires_only_the_latest_late() {
    struct StalledIntervalHost;
    struct StalledCronHost;
    const PERIOD_MS: u64 = 300;
    const SECOND_MS: u64 = 1_000;
    const STALL: Duration = Duration::from_millis(2_500);
    // What a fire may take past its occurrence; a stale occurrence comes later
    // than a period by far more than this.
    const SLACK_MS: u64 = 250;

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(SharedLock::default());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<StalledIntervalHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "StalledIntervalHost",
            method: "tick",
            trigger: Trigger::Interval(Duration::from_millis(PERIOD_MS)),
            run: tick_stalled_interval,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .attach_meta::<StalledCronHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "StalledCronHost",
            method: "tick",
            trigger: Trigger::Cron {
                expr: "* * * * * *",
                tz: None,
            },
            run: tick_stalled_cron,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    // The clock jumps without the scheduler polled, as a paused replica's does:
    // every occurrence due meanwhile is overdue at once.
    let stall_ends = wall_ms() + u64::try_from(STALL.as_millis()).expect("in range");
    tokio::time::advance(STALL).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    for (host, fires, period_ms) in [
        ("StalledIntervalHost", &STALLED_INTERVAL_FIRES, PERIOD_MS),
        ("StalledCronHost", &STALLED_CRON_FIRES, SECOND_MS),
    ] {
        let occurrences: Vec<u64> = logs
            .find("nest_rs::operation", "schedule.tick")
            .into_iter()
            .filter(|line| line.field("provider").as_deref() == Some(host))
            .map(|line| {
                line.field("occurrence")
                    .and_then(|instant| instant.parse().ok())
                    .unwrap_or_else(|| panic!("a job firing once carries its occurrence: {line:?}"))
            })
            .collect();
        let fires = fires.lock().expect("lock").clone();
        assert!(
            fires.iter().any(|fired| *fired >= stall_ends),
            "{host} fires again once the stall ends: {fires:?}",
        );
        for (occurrence, fired) in occurrences.iter().zip(&fires) {
            assert!(
                fired.saturating_sub(*occurrence) < period_ms + SLACK_MS,
                "{host} fired its occurrence at {occurrence} only at {fired}, a period or more \
                 late: occurrences {occurrences:?} fired at {fires:?}",
            );
        }
    }
}

static LONG_RUNS: AtomicU64 = AtomicU64::new(0);

fn tick_longer_than_its_period(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        LONG_RUNS.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    })
}

/// With a 700 ms run every 200 ms, the ticks due at 200 and 400 fire, the second
/// late, and 600 and 800 are skipped before the one due at 1000 fires.
#[tokio::test(start_paused = true)]
async fn ticks_a_long_run_overran_on_every_replica_are_skipped_and_counted_aloud() {
    struct LongRunHost;
    const PERIOD: Duration = Duration::from_millis(200);

    let logs = LogCapture::install();
    let container = crate::hermetic()
        .attach_meta::<LongRunHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "LongRunHost",
            method: "crunch",
            trigger: Trigger::Interval(PERIOD),
            run: tick_longer_than_its_period,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::Each,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler.configure(&container).await.expect("configures");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(1_800)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert!(LONG_RUNS.load(Ordering::SeqCst) >= 3);
    let skipped: Vec<_> = logs
        .find(
            "nest_rs::schedule",
            "occurrences skipped: they fell due while the previous one was claimed or run",
        )
        .into_iter()
        .filter(|event| event.field("provider").as_deref() == Some("LongRunHost"))
        .collect();
    let event = skipped
        .first()
        .unwrap_or_else(|| panic!("the overrun ticks are reported: {:#?}", logs.events()));
    assert_eq!(event.level, "warn");
    let count: u64 = event
        .field("skipped")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("the event counts them: {event:?}"));
    assert_eq!(
        count, 2,
        "the late tick fired and is not skipped; the two after it are: {event:?}",
    );
    assert_eq!(
        event.field("claimed_elsewhere"),
        None,
        "a job firing on every replica has no claim to ask about: {event:?}",
    );
}

static SLIGHTLY_LONG_RUNS: AtomicU64 = AtomicU64::new(0);

fn tick_slightly_longer_than_its_period(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        SLIGHTLY_LONG_RUNS.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(103)).await;
        Ok(())
    })
}

/// The clock is paused so the lateness is the run's alone: a loaded executor's
/// own delay would add up to a period.
#[tokio::test(start_paused = true)]
async fn a_run_just_over_its_period_is_late_on_every_tick_and_skips_none() {
    struct SlightlyLongHost;

    let logs = LogCapture::install();
    let container = crate::hermetic()
        .attach_meta::<SlightlyLongHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "SlightlyLongHost",
            method: "crunch",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_slightly_longer_than_its_period,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::Each,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler.configure(&container).await.expect("configures");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert!(
        SLIGHTLY_LONG_RUNS.load(Ordering::SeqCst) >= 3,
        "every tick fires, late"
    );
    let skips: Vec<_> = logs
        .find(
            "nest_rs::schedule",
            "occurrences skipped: they fell due while the previous one was claimed or run",
        )
        .into_iter()
        .filter(|event| event.field("provider").as_deref() == Some("SlightlyLongHost"))
        .collect();
    assert!(skips.is_empty(), "no tick was skipped: {skips:#?}");
}

/// A lock whose claims and whose answers about claims each take three periods,
/// and which grants nothing: another replica claimed every occurrence it asks for,
/// and nobody claimed the ones in between.
#[derive(Default)]
struct StallingLock {
    claims: std::sync::Mutex<Vec<u64>>,
}

#[async_trait::async_trait]
impl OccurrenceLock for StallingLock {
    async fn claim(&self, occurrence: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError> {
        self.claims
            .lock()
            .expect("lock")
            .push(instant_of(&occurrence.token));
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(OccurrenceClaim::ClaimedElsewhere)
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(false)
    }
}

/// A lock that records the token it is handed at **both** sites, and stalls its
/// claim so the overrun path runs.
#[derive(Default)]
struct RecordingLock {
    claimed_at_claim: std::sync::Mutex<Vec<String>>,
    asked_at_overrun: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl OccurrenceLock for RecordingLock {
    async fn claim(&self, occurrence: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError> {
        self.claimed_at_claim
            .lock()
            .expect("lock")
            .push(occurrence.token.clone());
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, occurrence: &str) -> Result<bool, OccurrenceLockError> {
        self.asked_at_overrun
            .lock()
            .expect("lock")
            .push(occurrence.to_owned());
        Ok(false)
    }
}

/// Every other lock double in this suite ignores its argument.
#[tokio::test(start_paused = true)]
async fn the_claim_and_the_overrun_check_are_asked_one_token_shape() {
    struct RecordedHost;
    const PROVIDER: &str = "RecordedHost";
    const METHOD: &str = "sweep";

    let lock = Arc::new(RecordingLock::default());
    let bound: Arc<dyn OccurrenceLock> = lock.clone();
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(bound)
        .attach_meta::<RecordedHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: PROVIDER,
            method: METHOD,
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_noop,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    let claimed = lock.claimed_at_claim.lock().expect("lock").clone();
    let asked = lock.asked_at_overrun.lock().expect("lock").clone();
    assert!(!claimed.is_empty(), "the job claimed an occurrence");
    assert!(
        !asked.is_empty(),
        "a stalled claim overruns, so the overrun check ran: {asked:?}"
    );

    let identity = format!("{}:{PROVIDER}:{METHOD}:", crate_name());
    for token in claimed.iter().chain(&asked) {
        let instant = token
            .strip_prefix(&identity)
            .unwrap_or_else(|| panic!("both sites ask `{identity}<instant>`: {token}"));
        assert!(
            instant.parse::<u64>().is_ok(),
            "the instant is last: {token}"
        );
    }
}

/// Counted under any of the report's heads: the report cut short at shutdown
/// counts its instants `unanswered`.
#[tokio::test(start_paused = true)]
async fn every_instant_of_a_job_firing_once_is_claimed_or_counted() {
    struct StalledHost;
    const PERIOD_MS: u64 = 100;

    let logs = LogCapture::install();
    let lock = Arc::new(StallingLock::default());
    let bound: Arc<dyn OccurrenceLock> = lock.clone();
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(bound)
        .attach_meta::<StalledHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "StalledHost",
            method: "sweep",
            trigger: Trigger::Interval(Duration::from_millis(PERIOD_MS)),
            run: tick_noop,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    let claims = lock.claims.lock().expect("lock").clone();
    assert!(claims.len() >= 3, "claims: {claims:?}");
    let counted: std::collections::BTreeMap<u64, u64> = logs
        .find(
            "nest_rs::schedule",
            "occurrences skipped: they fell due while the previous one was claimed or run",
        )
        .into_iter()
        .filter(|event| event.field("provider").as_deref() == Some("StalledHost"))
        .map(|event| {
            let number = |field: &str| {
                event
                    .field(field)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or_else(|| panic!("the event carries `{field}`: {event:?}"))
            };
            let named = ["skipped", "claimed_elsewhere", "unanswered", "unchecked"]
                .into_iter()
                .map(number)
                .sum();
            (number("occurrence"), named)
        })
        .collect();
    for pair in claims.windows(2) {
        let between = (pair[1] - pair[0]) / PERIOD_MS - 1;
        assert_eq!(
            counted.get(&pair[0]).copied().unwrap_or(0),
            between,
            "the occurrences between the claims at {} and {} are counted once each: {counted:?}",
            pair[0],
            pair[1],
        );
    }
}

/// A lock granting its first claim after many periods, and unable to answer who
/// claimed the occurrences that claim overran.
#[derive(Default)]
struct UnanswerableLock {
    claimed: AtomicBool,
}

#[async_trait::async_trait]
impl OccurrenceLock for UnanswerableLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        if self.claimed.swap(true, Ordering::SeqCst) {
            return Ok(OccurrenceClaim::ClaimedElsewhere);
        }
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        Err(unreachable_lock())
    }
}

#[tokio::test(start_paused = true)]
async fn an_overrun_the_lock_cannot_answer_about_is_counted_unanswered_and_unchecked() {
    struct UnanswerableHost;

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(UnanswerableLock::default());
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<UnanswerableHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "UnanswerableHost",
            method: "sweep",
            trigger: Trigger::Interval(Duration::from_millis(5)),
            run: tick_noop,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    let reports = logs
        .find(
            "nest_rs::schedule",
            "occurrences skipped: they fell due while the previous one was claimed or run",
        )
        .into_iter()
        .filter(|event| event.field("provider").as_deref() == Some("UnanswerableHost"))
        .collect::<Vec<_>>();
    let event = reports
        .first()
        .unwrap_or_else(|| panic!("the overrun is reported: {:#?}", logs.events()));
    assert_eq!(
        event.field("unanswered").as_deref(),
        Some("100"),
        "{event:?}"
    );
    assert_eq!(event.field("skipped").as_deref(), Some("0"), "{event:?}");
    assert_eq!(
        event.field("claimed_elsewhere").as_deref(),
        Some("0"),
        "{event:?}"
    );
    assert!(
        event
            .field("unchecked")
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| n >= 100),
        "a 1.1 s claim over a 5 ms period overruns about 220: {event:?}",
    );
    let why = logs
        .find(
            "nest_rs::schedule",
            "occurrence lock could not answer whether an overrun occurrence was claimed",
        )
        .into_iter()
        .find(|event| event.field("provider").as_deref() == Some("UnanswerableHost"))
        .unwrap_or_else(|| panic!("the report says why it could not ask: {:#?}", logs.events()));
    assert_eq!(why.level, "warn");
    assert_eq!(
        why.field("error").as_deref(),
        Some("the lock store is unreachable: connection refused by redis:6379"),
        "the failure and the cause beneath it: {why:?}",
    );
}

/// A lock that panics while claiming — a third-party store's client with a bug.
struct PanickingLock {
    claims: AtomicU64,
}

#[async_trait::async_trait]
impl OccurrenceLock for PanickingLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        self.claims.fetch_add(1, Ordering::SeqCst);
        panic!("the lock store's client panicked");
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        panic!("the lock store's client panicked");
    }
}

static BEHIND_A_PANICKING_LOCK: AtomicU64 = AtomicU64::new(0);

fn tick_behind_a_panicking_lock(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        BEHIND_A_PANICKING_LOCK.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

/// Single-thread runtime: `LogCapture` is thread-local.
#[tokio::test(start_paused = true)]
async fn a_lock_that_panics_skips_the_occurrence_and_the_schedule_goes_on() {
    struct PanickedLockHost;

    let logs = LogCapture::install();
    let lock = Arc::new(PanickingLock {
        claims: AtomicU64::new(0),
    });
    let bound: Arc<dyn OccurrenceLock> = lock.clone();
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(bound)
        .attach_meta::<PanickedLockHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "PanickedLockHost",
            method: "once",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_behind_a_panicking_lock,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok");

    assert!(
        lock.claims.load(Ordering::SeqCst) >= 2,
        "the job claims again after its lock panicked (claims: {})",
        lock.claims.load(Ordering::SeqCst),
    );
    assert_eq!(
        BEHIND_A_PANICKING_LOCK.load(Ordering::SeqCst),
        0,
        "an occurrence nobody claimed never fires",
    );
    let panicked = logs.find(
        "nest_rs::schedule",
        "occurrence skipped: its lock panicked while claiming it",
    );
    let event = panicked
        .first()
        .unwrap_or_else(|| panic!("the panic is reported: {:#?}", logs.events()));
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the lock store's client panicked")
    );
    logs.expect_none(
        "nest_rs::schedule",
        "occurrence skipped: its lock could not be claimed",
    );
}

static PANICKED_BEFORE_ITS_FUTURE: AtomicU64 = AtomicU64::new(0);

fn tick_panicking_before_its_future(_: &Container) -> RunFuture<'_> {
    PANICKED_BEFORE_ITS_FUTURE.fetch_add(1, Ordering::SeqCst);
    panic!("boom before the future");
}

#[tokio::test(start_paused = true)]
async fn a_run_panicking_before_its_future_keeps_its_schedule() {
    struct EarlyPanicHost;

    let container = crate::hermetic()
        .attach_meta::<EarlyPanicHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "EarlyPanicHost",
            method: "panics_early",
            trigger: Trigger::Interval(Duration::from_millis(100)),
            run: tick_panicking_before_its_future,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::PerAttempt,
            replicas: Replicas::Each,
            key: None,
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
        .expect("serve task joins despite the panics")
        .expect("serve returns Ok despite the panics");

    assert!(
        PANICKED_BEFORE_ITS_FUTURE.load(Ordering::SeqCst) >= 2,
        "the job fires again after a run that panicked before its future (runs: {})",
        PANICKED_BEFORE_ITS_FUTURE.load(Ordering::SeqCst),
    );
}

/// An error whose text panics as it is written: the one panic a lock's answer can
/// still raise outside the catch around the claim, as its `warn` is filed.
#[derive(Debug)]
struct UnwritableError;

impl std::fmt::Display for UnwritableError {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("the lock error's text panicked")
    }
}

impl std::error::Error for UnwritableError {}

/// A lock whose every error panics as it is written.
struct UnwritableLock;

#[async_trait::async_trait]
impl OccurrenceLock for UnwritableLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        Err(OccurrenceLockError::new(UnwritableError))
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        Err(OccurrenceLockError::new(UnwritableError))
    }
}

fn tick_behind_an_unwritable_lock(_: &Container) -> RunFuture<'_> {
    Box::pin(async { Ok(()) })
}

/// Single-thread runtime: `LogCapture` is thread-local.
#[tokio::test(start_paused = true)]
async fn a_schedule_whose_every_job_died_keeps_serving_until_shutdown() {
    struct DoomedHost;

    let logs = LogCapture::install();
    let lock: Arc<dyn OccurrenceLock> = Arc::new(UnwritableLock);
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(lock)
        .attach_meta::<DoomedHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "DoomedHost",
            method: "once",
            trigger: Trigger::Interval(Duration::from_millis(50)),
            run: tick_behind_an_unwritable_lock,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        !serving.is_finished(),
        "the schedule stopped serving before any shutdown"
    );
    let stopped = logs.find(
        "nest_rs::schedule",
        "scheduled job stopped: its schedule panicked, and the job will not run again",
    );
    assert_eq!(
        stopped.len(),
        1,
        "the loop's end is named once: {:#?}",
        logs.events()
    );
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok at shutdown");
}

/// A lock whose claims never answer — a backend holding every command, a
/// network dropping them without a reset — and which says when one was sent.
#[derive(Default)]
struct HungLock {
    sent: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl OccurrenceLock for HungLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        self.sent.notify_one();
        std::future::pending().await
    }

    async fn claimed(&self, _occurrence: &str) -> Result<bool, OccurrenceLockError> {
        std::future::pending().await
    }
}

struct HungHost;

/// How long `serve` took to return after shutdown, with a claim in flight.
///
/// Measured on the runtime's clock, which a paused test moves only when every
/// task waits on a timer: a loop still waiting on its claim shows up as the time
/// to that claim's stale threshold, and a loop that let go shows up as none.
async fn shut_down_while_a_claim_hangs(
    method: &'static str,
    trigger: Trigger,
    run: RunFn,
) -> Duration {
    let lock = Arc::new(HungLock::default());
    let shared: Arc<dyn OccurrenceLock> = lock.clone();
    let container = crate::hermetic()
        .provide_dyn::<dyn OccurrenceLock>(shared)
        .attach_meta::<HungHost, CronJobMeta>(CronJobMeta {
            origin: module_path!(),
            provider: "HungHost",
            method,
            trigger,
            run,
            timeout: JOB_TIMEOUT,
            transaction: JobTransaction::Pool,
            replicas: Replicas::One,
            key: None,
        })
        .build();
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("configures with a lock bound");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));

    lock.sent.notified().await;
    let asked = tokio::time::Instant::now();
    cancel.cancel();
    serving
        .await
        .expect("serve task joins")
        .expect("serve returns Ok at shutdown");
    asked.elapsed()
}

fn assert_abandoned_once_at_shutdown(logs: &LogCapture, method: &str) {
    let abandoned = logs.expect_one(
        "nest_rs::schedule",
        "occurrence skipped: shutdown was asked for before its lock answered the claim",
    );
    assert_eq!(abandoned.level, "warn");
    assert_eq!(abandoned.field("provider").as_deref(), Some("HungHost"));
    assert_eq!(abandoned.field("method").as_deref(), Some(method));
    assert!(
        abandoned
            .field("occurrence")
            .is_some_and(|ms| ms.parse::<u64>().is_ok()),
        "the line names the occurrence it was claiming: {abandoned:?}",
    );
    logs.expect_none(
        "nest_rs::schedule",
        "occurrence skipped: its lock did not answer the claim before the occurrence went stale",
    );
}

static HUNG_INTERVAL_HITS: AtomicU64 = AtomicU64::new(0);

fn tick_hung_interval(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        HUNG_INTERVAL_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

#[tokio::test(start_paused = true)]
async fn shutdown_lets_go_of_an_interval_claim_its_lock_never_answers() {
    let logs = LogCapture::install();

    let waited = shut_down_while_a_claim_hangs(
        "sweep",
        Trigger::Interval(Duration::from_millis(100)),
        tick_hung_interval,
    )
    .await;

    assert_eq!(
        waited,
        Duration::ZERO,
        "shutdown returned without waiting on the lock"
    );
    assert_eq!(
        HUNG_INTERVAL_HITS.load(Ordering::SeqCst),
        0,
        "nothing fires once shutdown is asked for",
    );
    assert_abandoned_once_at_shutdown(&logs, "sweep");
}

static HUNG_CRON_HITS: AtomicU64 = AtomicU64::new(0);

fn tick_hung_cron(_: &Container) -> RunFuture<'_> {
    Box::pin(async {
        HUNG_CRON_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
}

/// A cron hold runs to the gap to its next occurrence: most of a day for a daily
/// job.
#[tokio::test(start_paused = true)]
async fn shutdown_lets_go_of_a_daily_cron_claim_its_lock_never_answers() {
    let logs = LogCapture::install();

    let waited = shut_down_while_a_claim_hangs(
        "close_the_day",
        Trigger::Cron {
            expr: CronExpression::EVERY_DAY_AT_MIDNIGHT,
            tz: None,
        },
        tick_hung_cron,
    )
    .await;

    assert_eq!(
        waited,
        Duration::ZERO,
        "shutdown returned without waiting on the lock"
    );
    assert_eq!(
        HUNG_CRON_HITS.load(Ordering::SeqCst),
        0,
        "nothing fires once shutdown is asked for",
    );
    assert_abandoned_once_at_shutdown(&logs, "close_the_day");
}
