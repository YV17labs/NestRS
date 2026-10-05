//! What the operation span reports about the request it served.
//!
//! These are the two OpenTelemetry HTTP conventions that need an answer only the
//! **router** has, which is why they are asserted through a mounted app rather
//! than as a unit: `http.route` must be the low-cardinality template, and the
//! exported span name must be `{method} {route}`.
//!
//! Both are read off the span rather than off the response, because that is
//! where they live — and a field declared and never recorded is absent here,
//! which is what makes the assertion mean something.

use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use nest_rs_testing::LogCapture;
use poem::http::StatusCode;
use tokio::sync::Notify;

use crate::boot;

#[controller(path = "/orgs")]
struct OrgsController;

#[routes]
impl OrgsController {
    /// Parameterised on purpose: the raw path and the template differ here, and
    /// nowhere else can tell them apart.
    #[get("/:org/members/:id")]
    #[public]
    async fn member(&self, org: poem::web::Path<String>, id: poem::web::Path<String>) -> String {
        format!("{}/{}", org.0, id.0)
    }
}

/// Told when the waiting route has started, so the request is dropped while
/// its handler runs.
static WAITING: Notify = Notify::const_new();

#[controller(path = "/jobs")]
struct JobsController;

#[routes]
impl JobsController {
    /// Waits on something that never comes: only dropping the request ends it.
    #[get("/:id/wait")]
    #[public]
    async fn wait(&self, _id: poem::web::Path<String>) -> &'static str {
        WAITING.notify_one();
        std::future::pending::<()>().await;
        "never"
    }

    #[get("/:id/done")]
    #[public]
    async fn done(&self, _id: poem::web::Path<String>) -> &'static str {
        "done"
    }

    #[get("/:id/broken")]
    #[public]
    async fn broken(&self, _id: poem::web::Path<String>) -> poem::Result<&'static str> {
        Err(poem::Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }

    #[get("/:id/missing")]
    #[public]
    async fn missing(&self, _id: poem::web::Path<String>) -> poem::Result<&'static str> {
        Err(poem::Error::from_status(StatusCode::NOT_FOUND))
    }
}

#[module(providers = [OrgsController, JobsController])]
struct OrgsModule;

/// `http.route` is what a backend groups latency and error rates on, so it has
/// to be the template. With the addressed path there instead, every identifier
/// is its own group and the aggregate says nothing — which is the state this
/// replaced.
#[tokio::test]
async fn the_span_reports_the_route_template_and_the_path_separately() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/members/42").send().await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(
        span.field("http.route").as_deref(),
        Some("/orgs/:org/members/:id"),
        "the template, never the addressed path: {:?}",
        span.fields,
    );
    assert_eq!(
        span.field("url.path").as_deref(),
        Some("/orgs/acme/members/42"),
        "and the addressed path keeps its own conventional field: {:?}",
        span.fields,
    );
}

/// The span names no client: an address is personal data, and a span is kept by
/// whatever collector receives it.
#[tokio::test]
async fn the_span_carries_no_client_address() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/members/42").send().await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(span.field("client.address"), None, "{:?}", span.fields);
}

/// `tracing` fixes a span's name to a literal, so one name would have to serve
/// every route and a trace list would render the whole deployment as a single
/// line. `otel.name` is the override an exporter reads.
#[tokio::test]
async fn the_exported_span_is_named_method_and_route() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/members/42").send().await;

    assert_eq!(
        logs.expect_span("nest_rs::http", "http.request")
            .field("otel.name")
            .as_deref(),
        Some("GET /orgs/:org/members/:id"),
    );
}

/// A request that matched nothing has no template, and the conventions' fallback
/// is the method alone. Naming it after the URL instead is how one scanner fills
/// a tracing backend with junk span names — so the absence is asserted, not
/// merely tolerated.
#[tokio::test]
async fn an_unmatched_request_is_named_by_its_method_alone() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/nothing-here").send().await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(span.field("otel.name").as_deref(), Some("GET"));
    assert_eq!(
        span.field("http.route"),
        None,
        "no route matched, so the field an aggregate groups on stays empty: {:?}",
        span.fields,
    );
}

/// A request dropped before it answered — its client reset the connection, or
/// the shutdown window closed on it — exports its span like an answered one,
/// named for the route the router had matched: the router answers before the
/// handler runs, and the drop takes only the response. Before, the name was read
/// off the response alone, so every cancelled request of a deployment exported
/// under the literal `http.request` with no `http.route`.
///
/// And it exports failed, in the conventions' terms: `error.type` is the word
/// its line files under `outcome`, and the span's status is `Error`.
#[tokio::test]
async fn a_request_dropped_before_it_answers_exports_its_span_under_its_route_and_failed() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    let dropped = tokio::spawn(async move { client.get("/jobs/7/wait").send().await });
    WAITING.notified().await;
    dropped.abort();
    let _ = dropped.await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(
        span.field("http.route").as_deref(),
        Some("/jobs/:id/wait"),
        "{:?}",
        span.fields,
    );
    assert_eq!(
        span.field("otel.name").as_deref(),
        Some("GET /jobs/:id/wait"),
        "{:?}",
        span.fields,
    );
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    assert_eq!(
        span.field("http.response.status_code"),
        None,
        "nothing was answered: {:?}",
        span.fields,
    );
}

/// A `5xx` is the one status class the HTTP conventions read as a failed server
/// operation: `error.type` is the status code, as a string, and the status is
/// `Error`. A `4xx` is the caller's error and an answered `2xx` succeeded — both
/// leave the two fields unset.
#[tokio::test]
async fn only_a_server_error_exports_its_span_failed() {
    for (path, status, failed) in [
        ("/jobs/7/broken", StatusCode::INTERNAL_SERVER_ERROR, true),
        ("/jobs/7/missing", StatusCode::NOT_FOUND, false),
        ("/jobs/7/done", StatusCode::OK, false),
    ] {
        let logs = LogCapture::install();
        let client = boot::<OrgsModule>().await;

        client.get(path).send().await.assert_status(status);

        let span = logs.expect_span("nest_rs::http", "http.request");
        let code = status.as_u16().to_string();
        assert_eq!(
            span.field("error.type"),
            failed.then(|| code.clone()),
            "{path}: {:?}",
            span.fields,
        );
        assert_eq!(
            span.field("otel.status_code").as_deref(),
            failed.then_some("error"),
            "{path}: {:?}",
            span.fields,
        );
        assert_eq!(span.field("http.response.status_code"), Some(code));
    }
}
