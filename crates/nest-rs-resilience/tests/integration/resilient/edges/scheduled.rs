//! A decorated tick of a `#[scheduled]` block, attempted again within the one
//! run its trigger fires, and refused: the occurrence filed failed, the one
//! after it run.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
use nest_rs_core::{injectable, module};
use nest_rs_resilience::{
    BreakerState, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
};
use nest_rs_schedule::{Scheduler, scheduled};
use nest_rs_testing::{HeadlessApp, LogCapture};

use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing_for, retrying_once};

#[injectable]
struct RefreshTasks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[scheduled]
impl RefreshTasks {
    #[after("1s")]
    #[resilient(upstream, idempotent)]
    async fn refresh(&self) -> Result<(), ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(())
    }
}

#[module(
    imports = [ResilienceModule::for_root(retrying_once())],
    providers = [RefreshTasks, Attempts],
)]
struct DecoratedScheduleModule;

#[tokio::test(start_paused = true)]
async fn a_tick_failing_once_answers_on_its_retry_within_its_one_run() {
    let logs = LogCapture::install();
    let app = HeadlessApp::builder()
        .module::<DecoratedScheduleModule>()
        .build_headless()
        .await
        .expect("a decorated tick boots");
    let scheduler = app
        .spawn_transport(Scheduler::new())
        .await
        .expect("the scheduler configures against the app");
    tokio::time::sleep(Duration::from_secs(2)).await;
    scheduler.shutdown().await.expect("the scheduler stops");

    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    let ran = logs.expect_one(OPERATIONS, nest_rs_schedule::unit::TICK.name());
    assert_eq!(ran.field("outcome").as_deref(), Some(OK));
    logs.expect_none(nest_rs_schedule::TARGET, "scheduled job failed");
}

#[injectable]
struct SyncTasks {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
}

#[scheduled]
impl SyncTasks {
    #[every("1s")]
    #[resilient(upstream, timeout = "100ms")]
    async fn sync(&self) -> Result<(), ResilienceError<UpstreamDown>> {
        self.dependency.call().await
    }
}

/// What a tick refused with a `503` files.
const UNAVAILABLE: &str = "scheduled job failed; unavailable";

/// How long a refusal of the tick below lasts: past its first occurrence, and
/// no longer at its second.
const REFUSED_FOR: Duration = Duration::from_millis(1500);

#[module(
    imports = [ResilienceModule::for_root(refusing_for(REFUSED_FOR))],
    providers = [SyncTasks, Dependency],
)]
struct RefusedScheduleModule;

/// No client reads a tick's `503`: its refusal is the occurrence failed, filed
/// at `warn` as every edge files a `503`, and the schedule goes on.
#[tokio::test(start_paused = true)]
async fn a_tick_its_policy_refuses_is_filed_failed_and_its_next_occurrence_runs() {
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<RefusedScheduleModule>()
            .build_headless()
            .await
            .expect("a decorated tick boots");
        let tasks = app.container().get::<SyncTasks>().expect("provided");
        let mut refused = refusing.arrange_for(app.container(), REFUSED_FOR).await;
        let (upstream, dependency) = (
            Arc::clone(&refused.upstream),
            Arc::clone(&refused.dependency),
        );
        let mut events = upstream.events();
        let scheduler = app
            .spawn_transport(Scheduler::new())
            .await
            .expect("the scheduler configures against the app");

        tokio::time::sleep(Duration::from_millis(1500)).await;
        let mounted = logs.expect_one(nest_rs_schedule::TARGET, "mounted");
        let failed = logs.expect_one(nest_rs_schedule::TARGET, UNAVAILABLE);
        assert_eq!(failed.level, "warn", "{failed:?}");
        assert!(logs.events_at(tracing::Level::ERROR).is_empty());
        let ran = logs.expect_one(OPERATIONS, nest_rs_schedule::unit::TICK.name());
        assert_eq!(ran.field("outcome").as_deref(), Some(ERROR), "{ran:?}");
        for line in [&failed, &ran] {
            assert_eq!(line.field("host"), mounted.field("host"), "{line:?}");
            assert_eq!(line.field("method"), mounted.field("method"), "{line:?}");
        }
        assert_eq!(mounted.field("host").as_deref(), Some("SyncTasks"));
        assert_eq!(mounted.field("method").as_deref(), Some("sync"));
        refusing.told(&crate::told(&mut events), "SyncTasks", "sync");
        assert_eq!(dependency.called(), refusing.sent());
        assert_eq!(
            upstream.concurrency_running(),
            Some(refusing.held()),
            "the refused run holds no slot of the bulkhead"
        );

        // The breaker is left to its open time, the occurrence after it the
        // probe that closes it, and the rate to its refill.
        if !refusing.lapses() {
            refused.lift().await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        scheduler.shutdown().await.expect("the scheduler stops");

        let ticks = logs.find(OPERATIONS, nest_rs_schedule::unit::TICK.name());
        assert_eq!(ticks.len(), 2, "the occurrence after it fired: {ticks:#?}");
        assert_eq!(ticks[1].field("outcome").as_deref(), Some(OK), "{ticks:#?}");
        assert_eq!(dependency.called(), refusing.sent() + 1);
        assert_eq!(upstream.breaker_state(), Some(BreakerState::Closed));
        assert_eq!(
            logs.find(nest_rs_schedule::TARGET, UNAVAILABLE).len(),
            1,
            "the one refusal, filed once"
        );

        // The occurrence that ran ended the refusal: its open time or its
        // refill over, its stall lifted, the holding call dropped.
        let _refused = refusing.arrange(app.container()).await;
        assert_eq!(
            failed.field("error"),
            Some(refusing.chain(tasks.sync().await)),
            "{failed:?}"
        );
    }
}
