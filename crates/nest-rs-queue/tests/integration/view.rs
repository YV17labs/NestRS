//! A job attempt is a unit of the pipeline: the layers a `#[process]` method,
//! its `#[processor]` block and the app bind run around it, read the job
//! through `QueueView`, and settle it — a filter's problem by its class, an
//! acknowledgement without running it.

use std::any::TypeId;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::problem::code;
use nest_rs_core::{
    App, Caught, Container, ExceptionFilter, Interceptor, Layer, Next, Outcome, Problem,
    ReachableProviders, Transport, UnitContext, async_trait, injectable, interceptor, module,
};
use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::{
    BoundConsumer, JobId, QueueName, QueueView, QueueWorker, WIRE_FORMAT_VERSION, processor, queue,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::memory::{Memory, PLAIN};
use crate::{BARE, method};

/// A job id as a push mints it.
const JOB_ID: &str = "01890a5d-ac96-774b-bcce-b302099a8057";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ViewedCommand {
    file: String,
}

#[queue(name = "viewed", job = ViewedCommand)]
struct ViewedQueue;

#[queue(name = "acknowledged", job = ViewedCommand)]
struct AcknowledgedQueue;

#[queue(name = "unprocessable", job = ViewedCommand)]
struct UnprocessableQueue;

#[queue(name = "unavailable", job = ViewedCommand)]
struct UnavailableQueue;

#[queue(name = "forgiven", job = ViewedCommand)]
struct ForgivenQueue;

#[queue(name = "hosted", job = ViewedCommand)]
struct HostedQueue;

/// What one layer read of the job it ran around.
#[derive(Debug, Clone, PartialEq)]
struct Read {
    job_id: JobId,
    attempt: u32,
    queue: String,
    processor: &'static str,
    backend_id: Option<String>,
    unit: &'static str,
    bounded: bool,
}

static READ: Mutex<Vec<Read>> = Mutex::new(Vec::new());
static RAN: AtomicUsize = AtomicUsize::new(0);

/// Writes down what the job's view says, then runs the rest of the unit.
#[injectable]
#[derive(Default)]
struct Reading;

impl Layer for Reading {}

#[async_trait]
impl Interceptor for Reading {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let view = cx
            .switch_to::<QueueView>()
            .expect("a job attempt carries its queue view");
        let read = Read {
            job_id: view.attempt().job_id().clone(),
            attempt: view.attempt().number(),
            queue: view.attempt().queue().as_str().to_owned(),
            processor: view.processor(),
            backend_id: view.backend_id().map(str::to_owned),
            unit: cx.unit().name(),
            bounded: cx.deadline().is_some(),
        };
        READ.lock().expect("lock").push(read);
        next.run(cx).await
    }
}

/// Settles the job without running it: the inbox pattern.
#[injectable]
#[derive(Default)]
struct Acknowledging;

impl Layer for Acknowledging {}

#[async_trait]
impl Interceptor for Acknowledging {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if let Some(job) = cx.switch_to_mut::<QueueView>() {
            return Ok(job.acknowledge());
        }
        next.run(cx).await
    }
}

/// The domain refusal the methods below fail with.
#[derive(Debug, thiserror::Error)]
#[error("the clip is not a wave file")]
struct NotAWave;

/// Answers [`NotAWave`] with a `422`: the client's error, never cleared by a retry.
#[injectable]
#[derive(Default)]
struct Unprocessable;

impl Layer for Unprocessable {}

#[async_trait]
impl ExceptionFilter for Unprocessable {
    type Exception = NotAWave;

    async fn catch(&self, _: &NotAWave, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(422, code::INVALID_ARGUMENT))
    }
}

/// Answers [`NotAWave`] with a `503`: a condition another attempt may clear.
#[injectable]
#[derive(Default)]
struct Unavailable;

impl Layer for Unavailable {}

#[async_trait]
impl ExceptionFilter for Unavailable {
    type Exception = NotAWave;

    async fn catch(&self, _: &NotAWave, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(503, code::UNAVAILABLE))
    }
}

/// Answers every failure by acknowledging its job.
#[injectable]
#[derive(Default)]
struct Forgiving;

impl Layer for Forgiving {}

#[async_trait]
impl ExceptionFilter for Forgiving {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        match cx.switch_to_mut::<QueueView>() {
            Some(job) => Caught::Answered(job.acknowledge()),
            None => Caught::Declined,
        }
    }
}

#[injectable]
#[derive(Default)]
struct ViewedProcessor;

#[processor]
impl ViewedProcessor {
    #[process(queue = ViewedQueue, retries = 2)]
    #[use_interceptors(Reading)]
    async fn seen(&self, job: ViewedCommand) -> anyhow::Result<()> {
        let _ = job;
        RAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[process(queue = AcknowledgedQueue)]
    #[use_interceptors(Acknowledging)]
    async fn acknowledged(&self, job: ViewedCommand) -> anyhow::Result<()> {
        let _ = job;
        RAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[process(queue = UnprocessableQueue, retries = 3)]
    #[use_exception_filters(Unprocessable)]
    async fn unprocessable(&self, job: ViewedCommand) -> anyhow::Result<()> {
        let _ = job;
        Err(NotAWave.into())
    }

    #[process(queue = UnavailableQueue, retries = 3)]
    #[use_exception_filters(Unavailable)]
    async fn unavailable(&self, job: ViewedCommand) -> anyhow::Result<()> {
        let _ = job;
        Err(NotAWave.into())
    }

    #[process(queue = ForgivenQueue, retries = 3)]
    #[use_exception_filters(Forgiving)]
    async fn forgiven(&self, job: ViewedCommand) -> anyhow::Result<()> {
        let _ = job;
        Err(NotAWave.into())
    }
}

#[injectable]
#[derive(Default)]
struct HostedProcessor;

#[processor]
#[use_interceptors(Reading)]
impl HostedProcessor {
    #[process(queue = HostedQueue)]
    async fn hosted(&self, job: ViewedCommand) -> anyhow::Result<()> {
        let _ = job;
        RAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn delivery(queue: &'static str) -> Delivery {
    Delivery::new(
        &BARE,
        QueueName::new(queue).expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "id": JOB_ID, "attempt": 2, "payload": { "file": "a.wav" } }),
    )
    .with_backend_id("1700000000000-0")
}

fn providing() -> Container {
    Container::builder()
        .provide(ViewedProcessor)
        .provide(HostedProcessor)
        .provide(Reading)
        .provide(Acknowledging)
        .provide(Unprocessable)
        .provide(Unavailable)
        .provide(Forgiving)
        .build()
        .expect("the container builds")
}

fn read() -> Vec<Read> {
    READ.lock().expect("lock").clone()
}

#[tokio::test]
async fn an_interceptor_on_the_method_reads_the_job_its_attempt_and_its_processor() {
    let outcome = consume::attempt(
        method("ViewedProcessor::seen"),
        &mut delivery("viewed"),
        providing(),
        None,
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    assert_eq!(RAN.load(Ordering::SeqCst), 1);
    assert_eq!(
        read(),
        [Read {
            job_id: JobId::parse(JOB_ID).expect("a job id"),
            attempt: 2,
            queue: "viewed".to_owned(),
            processor: "ViewedProcessor::seen",
            backend_id: Some("1700000000000-0".to_owned()),
            unit: nest_rs_queue::unit::JOB.name(),
            bounded: true,
        }],
    );
}

#[tokio::test]
async fn an_interceptor_on_the_processor_block_runs_around_each_of_its_jobs() {
    let outcome = consume::attempt(
        method("HostedProcessor::hosted"),
        &mut delivery("hosted"),
        providing(),
        None,
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    assert_eq!(
        read().iter().map(|read| read.processor).collect::<Vec<_>>(),
        ["HostedProcessor::hosted"],
    );
}

#[tokio::test]
async fn an_interceptor_acknowledging_a_job_settles_it_without_running_it() {
    let logs = nest_rs_testing::LogCapture::install();
    let outcome = consume::attempt(
        method("ViewedProcessor::acknowledged"),
        &mut delivery("acknowledged"),
        providing(),
        None,
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    assert_eq!(RAN.load(Ordering::SeqCst), 0, "the method never ran");
    let said = logs.expect_one(nest_rs_queue::TARGET, "job acknowledged without running");
    assert_eq!(said.field("attempt").as_deref(), Some("2"));
}

#[tokio::test]
async fn a_filter_answering_a_client_error_dead_letters_the_job_at_once() {
    let mut first = Delivery::new(
        &BARE,
        QueueName::new("unprocessable").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": { "file": "a.mp3" } }),
    );
    let outcome = consume::attempt(
        method("ViewedProcessor::unprocessable"),
        &mut first,
        providing(),
        None,
    )
    .await;

    let AttemptOutcome::DeadLetter(error) = outcome else {
        panic!("a 422 is the client's error, which no retry clears: {outcome:?}");
    };
    assert!(!error.retryable);
    assert_eq!(
        nest_rs_core::error_message(&error),
        "the clip is not a wave file",
        "the dead letter keeps the failure's own chain",
    );
}

#[tokio::test]
async fn a_filter_answering_an_unavailable_service_retries_the_job() {
    let mut first = Delivery::new(
        &BARE,
        QueueName::new("unavailable").expect("a valid name"),
        json!({ "v": WIRE_FORMAT_VERSION, "payload": { "file": "a.mp3" } }),
    );
    let outcome = consume::attempt(
        method("ViewedProcessor::unavailable"),
        &mut first,
        providing(),
        None,
    )
    .await;

    assert!(
        matches!(outcome, AttemptOutcome::Retry { .. }),
        "a 503 invites a repeat: {outcome:?}"
    );
    assert_eq!(first.attempt(), 2, "the delivery counts the next attempt");
}

#[tokio::test]
async fn a_filter_acknowledging_a_failed_job_completes_it_and_says_so() {
    let logs = nest_rs_testing::LogCapture::install();
    let outcome = consume::attempt(
        method("ViewedProcessor::forgiven"),
        &mut delivery("forgiven"),
        providing(),
        None,
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    let said = logs.expect_one(
        nest_rs_queue::TARGET,
        "job failed; a layer acknowledged it, so it settles done and its writes rolled back",
    );
    assert_eq!(said.level, "warn");
    assert_eq!(
        said.field("error").as_deref(),
        Some("the clip is not a wave file")
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
        "the unit failed, whatever its job settled on",
    );
}

#[injectable]
#[derive(Default)]
struct AppReading;

impl Layer for AppReading {}

#[async_trait]
impl Interceptor for AppReading {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if let Some(job) = cx.switch_to::<QueueView>() {
            READ.lock().expect("lock").push(Read {
                job_id: job.attempt().job_id().clone(),
                attempt: job.attempt().number(),
                queue: job.attempt().queue().as_str().to_owned(),
                processor: job.processor(),
                backend_id: None,
                unit: "app",
                bounded: cx.deadline().is_some(),
            });
        }
        next.run(cx).await
    }
}

#[module(providers = [HostedProcessor, Reading, AppReading])]
struct ViewedModule;

#[tokio::test]
async fn the_apps_interceptors_run_around_every_job_outermost() {
    let builder = App::builder().module::<ViewedModule>();
    let app =
        nest_rs_core::__private::use_interceptors_global(builder, [interceptor::<AppReading>()])
            .build()
            .await
            .expect("the app boots");

    let outcome = consume::attempt(
        method("HostedProcessor::hosted"),
        &mut delivery("hosted"),
        app.container().clone(),
        None,
    )
    .await;

    assert!(matches!(outcome, AttemptOutcome::Ok), "{outcome:?}");
    assert_eq!(
        read().iter().map(|read| read.unit).collect::<Vec<_>>(),
        ["app", nest_rs_queue::unit::JOB.name()],
        "the app's interceptor, then the block's",
    );
}

#[module(providers = [Reading])]
struct LayersModule;

#[module(providers = [HostedProcessor])]
struct HostingModule;

#[module(imports = [LayersModule, HostingModule])]
struct SidewaysRoot;

#[tokio::test]
async fn a_layer_a_job_binds_from_a_module_its_host_does_not_import_fails_the_boot() {
    let refused = App::builder()
        .module::<SidewaysRoot>()
        .build()
        .await
        .err()
        .map(|error| format!("{error:#}"))
        .unwrap_or_default();
    for part in [
        "HostingModule",
        "HostedProcessor",
        "Reading",
        "LayersModule",
    ] {
        assert!(
            refused.contains(part),
            "the refusal names `{part}`: {refused}"
        );
    }
}

#[tokio::test]
async fn a_layer_a_job_binds_that_nothing_provides_fails_the_configure_naming_both() {
    let consumer = Memory::new(&PLAIN, Duration::from_secs(1)).consumer();
    let container = Container::builder()
        .provide(ReachableProviders(
            [TypeId::of::<HostedProcessor>()].into_iter().collect(),
        ))
        .provide(HostedProcessor)
        .provide(BoundConsumer::new(consumer))
        .build()
        .expect("the container builds");
    let refused = QueueWorker::new()
        .configure(&container)
        .await
        .err()
        .map(|error| format!("{error:#}"))
        .unwrap_or_default();
    for part in ["Reading", "HostedProcessor::hosted"] {
        assert!(
            refused.contains(part),
            "the refusal names `{part}`: {refused}"
        );
    }
}
