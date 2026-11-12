//! What the operation span reports about the request it served: `http.route`
//! and `{method} {route}` need the router, so they are asserted through a mounted app.

use nest_rs_core::module;
use nest_rs_http::StatusCode;
use nest_rs_http::{controller, routes};
use nest_rs_testing::LogCapture;
use tokio::sync::Notify;

use crate::boot;

#[controller(path = "/orgs")]
struct OrgsController;

#[routes]
impl OrgsController {
    /// Parameterised, so the raw path and the template differ.
    #[get("/{org}/members/{id}")]
    #[public]
    async fn member(
        &self,
        org: nest_rs_http::Path<String>,
        id: nest_rs_http::Path<String>,
    ) -> String {
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
    #[get("/{id}/wait")]
    #[public]
    async fn wait(&self, _id: nest_rs_http::Path<String>) -> &'static str {
        WAITING.notify_one();
        std::future::pending::<()>().await;
        "never"
    }

    #[get("/{id}/done")]
    #[public]
    async fn done(&self, _id: nest_rs_http::Path<String>) -> &'static str {
        "done"
    }

    #[get("/{id}/broken")]
    #[public]
    async fn broken(&self, _id: nest_rs_http::Path<String>) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }

    #[get("/{id}/missing")]
    #[public]
    async fn missing(&self, _id: nest_rs_http::Path<String>) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(StatusCode::NOT_FOUND))
    }
}

#[module(providers = [OrgsController, JobsController])]
struct OrgsModule;

/// `http.route` is the template a backend groups on, never the addressed path.
#[tokio::test]
async fn the_span_reports_the_route_template_and_the_path_separately() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/members/42").send().await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(
        span.field("http.route").as_deref(),
        Some("/orgs/{org}/members/{id}"),
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

/// The HTTP server conventions require `url.scheme` beside `url.path`.
#[tokio::test]
async fn the_span_reports_the_scheme_the_request_arrived_over() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/members/42").send().await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(
        span.field("url.scheme").as_deref(),
        Some("http"),
        "{:?}",
        span.fields
    );
}

/// The span names no client: an address is personal data.
#[tokio::test]
async fn the_span_carries_no_client_address() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/members/42").send().await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(span.field("client.address"), None, "{:?}", span.fields);
}

/// `tracing` fixes a span's name to a literal; `otel.name` is the override an
/// exporter reads.
#[tokio::test]
async fn the_exported_span_is_named_method_and_route() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client.get("/orgs/acme/members/42").send().await;

    assert_eq!(
        logs.expect_span("nest_rs::http", "http.request")
            .field("otel.name")
            .as_deref(),
        Some("GET /orgs/{org}/members/{id}"),
    );
}

/// A method outside the known set is `_OTHER` on the span, whose name then
/// reads `HTTP`, as the HTTP conventions spell it; the method as sent is the
/// conventions' `http.request.method_original`, required whenever the two
/// differ, and absent otherwise.
#[tokio::test]
async fn a_method_the_conventions_do_not_know_is_other() {
    let logs = LogCapture::install();
    let client = boot::<OrgsModule>().await;

    client
        .request(
            nest_rs_http::Method::from_bytes(b"get").expect("a method token"),
            "/orgs/acme/members/42",
        )
        .send()
        .await;

    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(
        span.field("http.request.method").as_deref(),
        Some("_OTHER"),
        "methods are case-sensitive: {:?}",
        span.fields,
    );
    assert_eq!(
        span.field("http.request.method_original").as_deref(),
        Some("get")
    );
    assert_eq!(
        span.field("otel.name").as_deref(),
        Some("HTTP /orgs/{org}/members/{id}"),
        "the route the router matched, never the method sent",
    );

    let logs = LogCapture::install();
    client.get("/orgs/acme/members/42").send().await;
    let span = logs.expect_span("nest_rs::http", "http.request");
    assert_eq!(span.field("http.request.method").as_deref(), Some("GET"));
    assert_eq!(
        span.field("http.request.method_original"),
        None,
        "{:?}",
        span.fields
    );
}

/// A claimed `traceparent` the span records beyond the conventions' set is
/// recorded only once it parsed: a malformed one is the client's free text.
#[tokio::test]
async fn a_claimed_traceparent_is_recorded_only_when_it_parsed() {
    for (claimed, recorded) in [
        (
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        ),
        ("traceparent-marked", None),
        (
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-marked",
            Some("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        ),
    ] {
        let logs = LogCapture::install();
        let client = boot::<OrgsModule>().await;

        client
            .get("/orgs/acme/members/42")
            .header("traceparent", claimed)
            .send()
            .await;

        let span = logs.expect_span("nest_rs::http", "http.request");
        assert_eq!(
            span.field("http.request.header.traceparent").as_deref(),
            recorded,
            "{claimed}: {:?}",
            span.fields,
        );
    }
}

/// A request that matched nothing is named for its method alone, never its URL.
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

/// A request dropped before it answered exports its span named for the route
/// the router matched, failed: `error.type` is its `outcome`, the status `Error`.
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
        Some("/jobs/{id}/wait"),
        "{:?}",
        span.fields,
    );
    assert_eq!(
        span.field("otel.name").as_deref(),
        Some("GET /jobs/{id}/wait"),
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

/// Only a `5xx` fails a server span: `error.type` is the status code and the
/// status `Error`; a `4xx` or `2xx` leaves both unset.
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
