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

#[module(providers = [OrgsController])]
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
