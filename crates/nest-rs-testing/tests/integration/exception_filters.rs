//! Typed [`ExceptionFilter`] dispatch across the three Layer-System scopes
//! (global, controller, method), TypeId dedup, fallthrough when a filter
//! does not match, and a clean separation from the unconditional
//! [`Filter`](nest_rs_filters::Filter).

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use nest_rs_core::{Layer, injectable, module};
use nest_rs_exception_filters::{ExceptionFilter, exception_filter};
use nest_rs_http::{controller, routes};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use tokio::sync::Mutex;

// --- shared observable state -------------------------------------------------

static DOMAIN_CATCH_COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    DOMAIN_CATCH_COUNTER.store(0, Ordering::SeqCst);
}

fn catches() -> usize {
    DOMAIN_CATCH_COUNTER.load(Ordering::SeqCst)
}

// --- typed errors ------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
#[error("domain failure: {0}")]
pub struct DomainError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("infrastructure failure: {0}")]
pub struct InfraError(pub String);

/// R12 L-2. Every handler above raises its error by hand
/// (`poem::Error::new(DomainError(…), status)`), which is the one shape that
/// needs no `ResponseError` — so the suite proved dispatch while never touching
/// the requirement a reader meets first. `/fundamentals/exception-filters/`
/// shipped 2.0.0 defining `DomainError` with neither this impl nor a handler,
/// and the natural `Result<_, DomainError>` return failed to compile with an
/// `E0277` on `IntoResult` that names neither. The status here is the
/// **default**; the filter replaces it, and
/// [`raising_the_error_the_ordinary_way_reaches_the_filter`] pins both halves.
impl poem::error::ResponseError for DomainError {
    fn status(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

// --- the filter under test ---------------------------------------------------

#[injectable]
#[derive(Default)]
struct DomainErrorFilter;

impl Layer for DomainErrorFilter {}

#[async_trait]
impl ExceptionFilter for DomainErrorFilter {
    type Exception = DomainError;

    async fn catch(&self, err: DomainError) -> poem::Response {
        DOMAIN_CATCH_COUNTER.fetch_add(1, Ordering::SeqCst);
        poem::Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .body(format!("caught: {err}"))
    }
}

// --- controllers, one per scope variant --------------------------------------

#[controller(path = "/global")]
struct GlobalScope;

#[routes]
impl GlobalScope {
    #[get("/domain")]
    async fn fail_domain_global(&self) -> poem::Result<&'static str> {
        Err(poem::Error::new(
            DomainError("bad input".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }

    #[get("/infra")]
    async fn fail_infra_global(&self) -> poem::Result<&'static str> {
        Err(poem::Error::new(
            InfraError("db down".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/ctrl")]
#[use_exception_filters(DomainErrorFilter)]
struct ControllerScope;

#[routes]
impl ControllerScope {
    #[get("/domain")]
    async fn fail_domain_ctrl(&self) -> poem::Result<&'static str> {
        Err(poem::Error::new(
            DomainError("at controller scope".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/method")]
struct MethodScope;

#[routes]
impl MethodScope {
    #[get("/domain")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn fail_domain_method(&self) -> poem::Result<&'static str> {
        Err(poem::Error::new(
            DomainError("at method scope".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/dup-global-method")]
struct DupGlobalMethod;

#[routes]
impl DupGlobalMethod {
    #[get("/domain")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn fail_domain_dup(&self) -> poem::Result<&'static str> {
        Err(poem::Error::new(
            DomainError("dup".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/dup-ctrl-method")]
#[use_exception_filters(DomainErrorFilter)]
struct DupCtrlMethod;

#[routes]
impl DupCtrlMethod {
    #[get("/domain")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn fail_domain_ctrl_method(&self) -> poem::Result<&'static str> {
        Err(poem::Error::new(
            DomainError("dup".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

/// The page's own shape: `Result<T, DomainError>` straight out of the handler,
/// no hand-built `poem::Error`. This is what the reader writes, so it is what
/// the suite has to compile — the `?`/`Err` path only exists because
/// `DomainError` implements `ResponseError`.
#[controller(path = "/ordinary")]
struct OrdinaryRaise;

#[routes]
impl OrdinaryRaise {
    #[get("/filtered")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn filtered(&self) -> Result<&'static str, DomainError> {
        Err(DomainError("boom".into()))
    }

    /// The same error with no filter bound — it renders from its own
    /// `ResponseError` status. The filter replaces that default; it does not
    /// create it, and the page now says so.
    #[get("/unfiltered")]
    async fn unfiltered(&self) -> Result<&'static str, DomainError> {
        Err(DomainError("boom".into()))
    }
}

#[module(providers = [
    DomainErrorFilter,
    GlobalScope,
    ControllerScope,
    MethodScope,
    DupGlobalMethod,
    DupCtrlMethod,
    OrdinaryRaise,
])]
struct ExceptionFiltersModule;

// --- tests -------------------------------------------------------------------

#[tokio::test]
async fn exception_filter_at_global_scope_catches_typed_error() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/global/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(catches(), 1);
}

#[tokio::test]
async fn exception_filter_at_controller_scope_catches_typed_error() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<ExceptionFiltersModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/ctrl/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(catches(), 1);
}

#[tokio::test]
async fn exception_filter_at_method_scope_catches_typed_error() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<ExceptionFiltersModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/method/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(catches(), 1);
}

#[tokio::test]
async fn raising_the_error_the_ordinary_way_reaches_the_filter() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<ExceptionFiltersModule>()
        .await
        .expect("boots");

    let filtered = app.http().get("/ordinary/filtered").send().await;
    filtered.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    filtered.assert_text("caught: domain failure: boom").await;
    assert_eq!(catches(), 1);

    // Unbind the filter and the same handler answers from the exception's own
    // `ResponseError` status — the two halves the page has to keep distinct.
    let unfiltered = app.http().get("/ordinary/unfiltered").send().await;
    unfiltered.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(catches(), 1, "the unfiltered route reaches no filter");
}

#[tokio::test]
async fn unmatched_error_falls_through_to_default_handler() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/global/infra").send().await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(catches(), 0, "DomainErrorFilter must not catch InfraError");
}

#[tokio::test]
async fn same_filter_global_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-global-method/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        catches(),
        1,
        "TypeId dedup: global + method redeclaration must still catch once",
    );
}

#[tokio::test]
async fn same_filter_at_all_three_scopes_catches_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    // Global + controller + method — every scope of the exception-filter
    // pool executes at the route site (typed catches sit closest to the
    // handler); the dedup still collapses the three declarations to one.
    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-ctrl-method/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        catches(),
        1,
        "global + controller + method declaration still catches exactly once",
    );
}
