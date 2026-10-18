//! A listener call is a unit of the pipeline: the layers an `#[on_event]`
//! method, its `#[listeners]` block and the app bind run around it, read the
//! call through `EventsView` under the emitter's trace, answer its failure, and
//! may skip it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use nest_rs_core::problem::code;
use nest_rs_core::{
    App, Caught, Correlation, ExceptionFilter, Interceptor, Layer, Next, Outcome, Problem, SpanId,
    TraceId, UnitContext, async_trait, current_span_id, current_trace_id, injectable, module,
    with_request_scope,
};
use nest_rs_events::{EventBus, EventsModule, EventsView, listeners};

#[derive(Clone)]
struct Priced {
    cents: u64,
}

#[derive(Clone)]
struct Withdrawn;

#[derive(Clone)]
struct Archived;

#[derive(Clone)]
struct Relisted;

#[derive(Clone)]
struct Reindexed;

/// What one layer read of the listener call it ran around.
#[derive(Debug, Clone, PartialEq)]
struct Read {
    event: &'static str,
    listener: &'static str,
    unit: &'static str,
    trace: Option<TraceId>,
    span: Option<SpanId>,
    bounded: bool,
}

static READ: Mutex<Vec<Read>> = Mutex::new(Vec::new());
static RAN: AtomicUsize = AtomicUsize::new(0);
static CAUGHT: AtomicUsize = AtomicUsize::new(0);

/// Writes down what the call's view says, then runs it.
#[injectable]
#[derive(Default)]
struct Reading;

impl Layer for Reading {}

#[async_trait]
impl Interceptor for Reading {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let view = cx
            .switch_to::<EventsView>()
            .expect("a listener call carries its events view");
        let read = Read {
            event: view.event(),
            listener: view.listener(),
            unit: cx.unit().name(),
            trace: current_trace_id(),
            span: current_span_id(),
            bounded: cx.deadline().is_some(),
        };
        READ.lock().expect("lock").push(read);
        next.run(cx).await
    }
}

/// Skips every call it runs around.
#[injectable]
#[derive(Default)]
struct Skipping;

impl Layer for Skipping {}

#[async_trait]
impl Interceptor for Skipping {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        if let Some(call) = cx.switch_to_mut::<EventsView>() {
            return Ok(call.skip());
        }
        next.run(cx).await
    }
}

/// Answers every failure by skipping the call: a failure the app expects.
#[injectable]
#[derive(Default)]
struct Expected;

impl Layer for Expected {}

#[async_trait]
impl ExceptionFilter for Expected {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        CAUGHT.fetch_add(1, Ordering::SeqCst);
        match cx.switch_to_mut::<EventsView>() {
            Some(call) => Caught::Answered(call.skip()),
            None => Caught::Declined,
        }
    }
}

/// Says every failure is the client's: a `422`.
#[injectable]
#[derive(Default)]
struct Unprocessable;

impl Layer for Unprocessable {}

#[async_trait]
impl ExceptionFilter for Unprocessable {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(422, code::INVALID_ARGUMENT))
    }
}

/// Says every failure is the server's: a `500`, as an app-wide filter mapping
/// errors to problems does.
#[injectable]
#[derive(Default)]
struct Internal;

impl Layer for Internal {}

#[async_trait]
impl ExceptionFilter for Internal {
    type Exception = dyn std::error::Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
        Caught::Problem(Problem::new(500, code::INTERNAL))
    }
}

#[injectable]
#[derive(Default)]
struct CatalogListeners;

#[listeners]
impl CatalogListeners {
    #[on_event]
    #[use_interceptors(Reading)]
    async fn on_priced(&self, event: Priced) {
        RAN.fetch_add(
            usize::try_from(event.cents).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
    }

    #[on_event]
    #[use_exception_filters(Expected)]
    async fn on_withdrawn(&self, _event: Withdrawn) -> anyhow::Result<()> {
        anyhow::bail!("the catalog is read-only during the import")
    }
}

#[injectable]
#[derive(Default)]
struct ArchiveListeners;

#[listeners]
#[use_interceptors(Skipping)]
impl ArchiveListeners {
    #[on_event]
    async fn on_archived(&self, _event: Archived) {
        RAN.fetch_add(1, Ordering::SeqCst);
    }
}

#[injectable]
#[derive(Default)]
struct IndexListeners;

#[listeners]
impl IndexListeners {
    #[on_event]
    #[use_exception_filters(Unprocessable)]
    async fn on_relisted(&self, _event: Relisted) -> anyhow::Result<()> {
        anyhow::bail!("the listing is not published yet")
    }

    #[on_event]
    #[use_exception_filters(Internal)]
    async fn on_reindexed(&self, _event: Reindexed) -> anyhow::Result<()> {
        anyhow::bail!("the search index refused the connection")
    }
}

#[module(
    imports = [EventsModule],
    providers = [
        CatalogListeners,
        ArchiveListeners,
        IndexListeners,
        Reading,
        Skipping,
        Expected,
        Unprocessable,
        Internal,
    ]
)]
struct CatalogModule;

async fn bus() -> anyhow::Result<(App, Arc<EventBus>)> {
    let app = App::builder().module::<CatalogModule>().build().await?;
    app.init().await?;
    let bus = app
        .container()
        .get::<EventBus>()
        .context("EventsModule provides the bus")?;
    Ok((app, bus))
}

#[tokio::test]
async fn an_interceptor_on_a_listener_reads_the_call_under_the_emitters_trace() {
    let (_app, bus) = bus().await.expect("the app boots");
    let (trace, span) = with_request_scope(None, Correlation::minted(None), async {
        let emitter = (current_trace_id(), current_span_id());
        bus.emit(Priced { cents: 7 }).await;
        emitter
    })
    .await;

    assert_eq!(RAN.load(Ordering::SeqCst), 7);
    let read = READ.lock().expect("lock").clone();
    let [read] = read.as_slice() else {
        panic!("one call read: {read:#?}");
    };
    assert_eq!(read.event, std::any::type_name::<Priced>());
    assert_eq!(read.listener, "CatalogListeners::on_priced");
    assert_eq!(read.unit, nest_rs_events::unit::DISPATCH.name());
    assert!(read.bounded, "a listener call runs to its timeout");
    assert_eq!(read.trace, trace, "one emit is one trace");
    assert!(
        read.span.is_some() && read.span != span,
        "the call is a child of the emitter"
    );
}

#[tokio::test]
async fn an_interceptor_skipping_a_call_settles_it_without_running_it() {
    let (_app, bus) = bus().await.expect("the app boots");
    bus.emit(Archived).await;
    assert_eq!(RAN.load(Ordering::SeqCst), 0, "the listener never ran");
}

#[tokio::test]
async fn a_failure_a_filter_answers_is_said_at_warn_and_never_at_error() {
    let (_app, bus) = bus().await.expect("the app boots");
    let logs = nest_rs_testing::LogCapture::install();
    bus.emit(Withdrawn).await;

    assert_eq!(CAUGHT.load(Ordering::SeqCst), 1);
    assert!(
        logs.find(
            nest_rs_events::TARGET,
            "event listener failed — dispatch continues with the next listener",
        )
        .is_empty(),
        "an answered failure is no error to page on: {:#?}",
        logs.events(),
    );
    let said = logs.expect_one(
        nest_rs_events::TARGET,
        "event listener failed; a layer answered it — dispatch continues with the next listener",
    );
    assert_eq!(said.level, "warn");
    assert_eq!(
        said.field("error").as_deref(),
        Some("the catalog is read-only during the import"),
    );
}

#[tokio::test]
async fn a_failure_a_filter_says_is_the_clients_is_said_at_warn() {
    let (_app, bus) = bus().await.expect("the app boots");
    let logs = nest_rs_testing::LogCapture::install();
    bus.emit(Relisted).await;

    let said = logs.expect_one(
        nest_rs_events::TARGET,
        "event listener failed; a layer answered it — dispatch continues with the next listener",
    );
    assert_eq!(said.level, "warn");
    assert!(
        logs.find(
            nest_rs_events::TARGET,
            "event listener failed — dispatch continues with the next listener",
        )
        .is_empty(),
        "a client's problem is no error to page on: {:#?}",
        logs.events(),
    );
}

#[tokio::test]
async fn a_failure_a_filter_says_is_the_servers_is_said_at_error() {
    let (_app, bus) = bus().await.expect("the app boots");
    let logs = nest_rs_testing::LogCapture::install();
    bus.emit(Reindexed).await;

    let said = logs.expect_one(
        nest_rs_events::TARGET,
        "event listener failed — dispatch continues with the next listener",
    );
    assert_eq!(said.level, "error");
    assert_eq!(
        said.field("error").as_deref(),
        Some("the search index refused the connection"),
    );
    assert!(
        logs.find(
            nest_rs_events::TARGET,
            "event listener failed; a layer answered it — dispatch continues with the next listener",
        )
        .is_empty(),
        "a server's problem is still an error: {:#?}",
        logs.events(),
    );
}

#[module(imports = [EventsModule], providers = [CatalogListeners, Expected])]
struct UnreadModule;

#[tokio::test]
async fn a_layer_a_listener_binds_that_no_module_provides_fails_the_boot_naming_both() {
    let refused = App::builder()
        .module::<UnreadModule>()
        .build()
        .await
        .err()
        .map(|error| format!("{error:#}"))
        .unwrap_or_default();
    for part in ["CatalogListeners", "Reading"] {
        assert!(
            refused.contains(part),
            "the refusal names `{part}`: {refused}"
        );
    }
}
