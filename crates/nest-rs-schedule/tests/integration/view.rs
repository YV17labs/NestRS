//! A scheduled tick is a unit of the pipeline: the layers a trigger, its
//! `#[scheduled]` block and the app bind run around it, read the tick through
//! `ScheduleView`, and may skip it.

use std::any::TypeId;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::{
    Caught, Container, ExceptionFilter, Interceptor, Layer, Next, Outcome, ReachableProviders,
    Transport, UnitContext, async_trait, injectable,
};
use nest_rs_schedule::{Replicas, ScheduleView, Scheduler, scheduled};
use tokio_util::sync::CancellationToken;

/// What one layer read of the tick it ran around.
#[derive(Debug, Clone, PartialEq)]
struct Read {
    job: String,
    replicas: Replicas,
    occurrence: bool,
    unit: &'static str,
    /// How far the tick's deadline sits past the moment the layer ran.
    deadline_in: Option<Duration>,
}

static READ: Mutex<Vec<Read>> = Mutex::new(Vec::new());
static RAN: AtomicUsize = AtomicUsize::new(0);
static CAUGHT: AtomicUsize = AtomicUsize::new(0);

/// Writes down what the tick's view says, then runs it.
#[injectable]
#[derive(Default)]
struct Reading;

impl Layer for Reading {}

#[async_trait]
impl Interceptor for Reading {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let view = cx
            .switch_to::<ScheduleView>()
            .expect("a tick carries its schedule view");
        let read = Read {
            job: view.job().to_owned(),
            replicas: view.replicas(),
            occurrence: view.occurrence().is_some(),
            unit: cx.unit().name(),
            deadline_in: cx
                .deadline()
                .map(|deadline| deadline - tokio::time::Instant::now()),
        };
        READ.lock().expect("lock").push(read);
        next.run(cx).await
    }
}

/// Skips every tick it runs around.
#[injectable]
#[derive(Default)]
struct Skipping;

impl Layer for Skipping {}

#[async_trait]
impl Interceptor for Skipping {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if let Some(tick) = cx.switch_to_mut::<ScheduleView>() {
            return Ok(tick.skip());
        }
        next.run(cx).await
    }
}

/// Answers every failure by skipping the tick: a failure the app expects.
#[injectable]
#[derive(Default)]
struct Expected;

impl Layer for Expected {}

#[async_trait]
impl ExceptionFilter for Expected {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        CAUGHT.fetch_add(1, Ordering::SeqCst);
        match cx.switch_to_mut::<ScheduleView>() {
            Some(tick) => Caught::Answered(tick.skip()),
            None => Caught::Declined,
        }
    }
}

#[injectable]
#[derive(Default)]
struct ViewedTasks;

#[scheduled]
impl ViewedTasks {
    #[after("100ms", timeout = "30s")]
    #[use_interceptors(Reading)]
    async fn warm(&self) -> anyhow::Result<()> {
        RAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[injectable]
#[derive(Default)]
struct SkippedTasks;

#[scheduled]
#[use_interceptors(Skipping)]
impl SkippedTasks {
    #[after("100ms")]
    async fn skipped(&self) -> anyhow::Result<()> {
        RAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[injectable]
#[derive(Default)]
struct FailingTasks;

#[scheduled]
impl FailingTasks {
    #[after("100ms")]
    #[use_exception_filters(Expected)]
    async fn failing(&self) -> anyhow::Result<()> {
        anyhow::bail!("the report source is closed for maintenance")
    }
}

/// Run every job of `host` the scheduler finds for half a second of paused
/// time.
async fn tick<H: Send + Sync + 'static>(host: H) -> Container {
    let container = Container::builder()
        .provide(ReachableProviders(
            [TypeId::of::<H>()].into_iter().collect(),
        ))
        .provide(host)
        .provide(Reading)
        .provide(Skipping)
        .provide(Expected)
        .build()
        .expect("the container builds");
    let mut scheduler = Scheduler::new();
    scheduler
        .configure(&container)
        .await
        .expect("the scheduler configures");
    let cancel = CancellationToken::new();
    let serving = tokio::spawn(Box::new(scheduler).serve(cancel.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel.cancel();
    serving
        .await
        .expect("serve joins")
        .expect("serve returns Ok");
    container
}

#[tokio::test(start_paused = true)]
async fn an_interceptor_on_a_trigger_reads_the_tick_and_its_deadline() {
    tick(ViewedTasks).await;

    assert_eq!(RAN.load(Ordering::SeqCst), 1);
    assert_eq!(
        *READ.lock().expect("lock"),
        [Read {
            job: "integration::ViewedTasks::warm".to_owned(),
            replicas: Replicas::Each,
            occurrence: false,
            unit: nest_rs_schedule::unit::TICK.name(),
            deadline_in: Some(Duration::from_secs(30)),
        }],
        "the paused clock holds the deadline at the trigger's timeout",
    );
}

#[tokio::test(start_paused = true)]
async fn an_interceptor_skipping_a_tick_settles_it_without_running_it() {
    let logs = nest_rs_testing::LogCapture::install();
    tick(SkippedTasks).await;

    assert_eq!(RAN.load(Ordering::SeqCst), 0, "the method never ran");
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_schedule::unit::TICK.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK)
    );
}

#[tokio::test(start_paused = true)]
async fn a_failure_a_filter_answers_is_said_at_warn_and_never_at_error() {
    let logs = nest_rs_testing::LogCapture::install();
    tick(FailingTasks).await;

    assert_eq!(CAUGHT.load(Ordering::SeqCst), 1);
    assert!(
        logs.find(nest_rs_schedule::TARGET, "scheduled job failed")
            .is_empty(),
        "an answered failure is no error to page on: {:#?}",
        logs.events(),
    );
    let said = logs.expect_one(
        nest_rs_schedule::TARGET,
        "scheduled job failed; a layer answered it",
    );
    assert_eq!(said.level, "warn");
    assert_eq!(
        said.field("error").as_deref(),
        Some("the report source is closed for maintenance"),
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_schedule::unit::TICK.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "the tick failed, whatever a layer answered",
    );
}

#[tokio::test]
async fn a_layer_a_tick_binds_that_nothing_provides_fails_the_configure_naming_both() {
    let container = Container::builder()
        .provide(ReachableProviders(
            [TypeId::of::<ViewedTasks>()].into_iter().collect(),
        ))
        .provide(ViewedTasks)
        .build()
        .expect("the container builds");
    let refused = Scheduler::new()
        .configure(&container)
        .await
        .err()
        .map(|error| format!("{error:#}"))
        .unwrap_or_default();
    for part in ["Reading", "ViewedTasks::warm"] {
        assert!(
            refused.contains(part),
            "the refusal names `{part}`: {refused}"
        );
    }
}
