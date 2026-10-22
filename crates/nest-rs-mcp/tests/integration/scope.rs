//! Ambient request state reaches a tool body across rmcp's spawned dispatch,
//! asserted on what the body saw rather than on the transport succeeding; a
//! `Scoped<T>` parameter resolves the operation's request-scoped provider, and
//! the boot checks it is reachable.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nest_rs_core::{AccessGraphError, App, MissingDependencyError, injectable, module};
use nest_rs_http::{BoxEndpoint, Endpoint, endpoint_fn};
use nest_rs_mcp::__private::{McpMount, endpoint};
use nest_rs_mcp::{
    CallToolResult, ContentBlock, McpError, Scoped, ServerHandler, mcp, tool, tool_handler,
    tool_router, tools,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::TestClient;
use nest_rs_testing::mcp::{call_method, call_tool, open_session, result};
use serde_json::json;
use tracing::Instrument;

/// Counts the instances built, so each one is told apart.
static BUILT: AtomicUsize = AtomicUsize::new(0);

/// Built once per request.
#[injectable(scope = request)]
struct Basket {
    serial: usize,
}

impl Default for Basket {
    fn default() -> Self {
        Self {
            serial: BUILT.fetch_add(1, Ordering::SeqCst),
        }
    }
}

#[module(providers = [Basket])]
struct BasketModule;

const BASKET: &str = "/mcp/basket";

#[mcp(path = "/mcp/basket")]
#[derive(Clone, Default)]
struct BasketTools;

#[tools]
impl BasketTools {
    /// Say which basket the operation reads, twice.
    #[tool]
    #[public]
    async fn peek(
        &self,
        first: Scoped<Basket>,
        second: Scoped<Basket>,
    ) -> Result<String, McpError> {
        Ok(format!("{} {}", first.serial, second.serial))
    }
}

#[module(imports = [crate::open_mcp(), BasketModule], providers = [BasketTools])]
struct BasketToolsModule;

#[tokio::test]
async fn a_scoped_parameter_resolves_the_operations_request_scoped_provider() {
    let app = TestApp::for_module::<BasketToolsModule>()
        .await
        .expect("a request-scoped provider its module imports boots");
    let session = open_session(app.http(), BASKET, None).await;
    let peek = || {
        call_method(
            app.http(),
            BASKET,
            &session,
            None,
            "tools/call",
            json!({ "name": "peek", "arguments": {} }),
        )
    };

    let first = result(&peek().await)["result"]["content"][0]["text"].clone();
    let second = result(&peek().await)["result"]["content"][0]["text"].clone();

    let serials = |said: &serde_json::Value| -> Vec<String> {
        said.as_str()
            .unwrap_or_else(|| panic!("the tool answered: {said}"))
            .split(' ')
            .map(str::to_owned)
            .collect()
    };
    let (first, second) = (serials(&first), serials(&second));
    assert_eq!(
        first[0], first[1],
        "one instance within an operation's request"
    );
    assert_ne!(first[0], second[0], "a fresh one for the next request");
}

/// The tools' module reaches the basket's only through a sibling.
#[module(providers = [BasketTools])]
struct StrayBasketToolsModule;

#[module(imports = [crate::open_mcp(), BasketModule, StrayBasketToolsModule])]
struct StrayBasketRoot;

#[tokio::test]
async fn a_scoped_parameter_whose_module_is_not_imported_fails_the_boot() {
    let refused = match App::builder().module::<StrayBasketRoot>().build().await {
        Ok(_) => panic!("an unimported `Scoped` target must fail the boot"),
        Err(refused) => refused,
    };
    let access = refused
        .downcast::<AccessGraphError>()
        .unwrap_or_else(|refused| panic!("not the access refusal: {refused:#}"));
    assert_eq!(access.consumer, "BasketTools");
    assert_eq!(access.owner, "BasketModule");
}

/// Offered by no module.
#[injectable(scope = request)]
#[derive(Default)]
struct Coupon;

#[mcp(path = "/mcp/coupon")]
#[derive(Clone, Default)]
struct CouponTools;

#[tools]
impl CouponTools {
    /// Redeem the coupon.
    #[tool]
    #[public]
    async fn redeem(&self, _coupon: Scoped<Coupon>) -> Result<String, McpError> {
        Ok("redeemed".to_owned())
    }
}

#[module(imports = [crate::open_mcp()], providers = [CouponTools])]
struct CouponToolsModule;

#[tokio::test]
async fn a_scoped_parameter_no_module_offers_fails_the_boot_naming_it() {
    let missing = match App::builder().module::<CouponToolsModule>().build().await {
        Ok(_) => panic!("a `Scoped` parameter no module offers must fail the boot"),
        Err(refused) => refused
            .downcast::<MissingDependencyError>()
            .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}")),
    };
    assert_eq!(missing.consumer, "CouponTools");
    assert!(
        missing.dependency.ends_with("::Coupon"),
        "{}",
        missing.dependency
    );
}

/// A request-scoped provider no module offers, reached by a host the boot
/// never checked: a container built by hand.
#[tokio::test]
async fn a_scoped_parameter_that_resolves_nothing_answers_opaquely_and_names_it_to_the_operator() {
    let logs = nest_rs_testing::LogCapture::install();
    let container = nest_rs_core::Container::builder()
        .build()
        .expect("the container builds");
    let inner = endpoint(McpMount::public(), || CouponTools);
    let scoped = endpoint_fn(move |req| {
        let inner = inner.clone();
        let scope = Arc::new(nest_rs_core::RequestScope::new(container.clone()));
        let correlation = nest_rs_core::Correlation::minted(None);
        async move { nest_rs_core::with_request_scope(Some(scope), correlation, inner.call(req)).await }
    });

    let body = call_tool(&TestClient::new(scoped), "/", "redeem", None).await;

    assert_eq!(
        result(&body)["error"]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{body}"
    );
    assert!(
        !body.contains("Coupon") && !body.contains("redeemed"),
        "{body}"
    );
    let failed = logs.expect_one(nest_rs_mcp::TARGET, "mcp operation failed");
    assert!(
        failed
            .field("error")
            .is_some_and(|chain| chain.contains("Coupon")),
        "the operator reads the provider it missed: {:?}",
        failed.fields,
    );
}

/// What the tool body observed: the span it ran under, and its trace.
type SeenSpan = Arc<Mutex<Option<(Option<&'static str>, Option<String>)>>>;

#[derive(Clone)]
struct SpanProbeTool {
    seen: SeenSpan,
}

#[tool_router]
impl SpanProbeTool {
    #[tool(description = "Record the span this tool body ran under.")]
    async fn probe_span(&self) -> Result<CallToolResult, McpError> {
        *self.seen.lock().expect("probe lock") = Some((
            tracing::Span::current().metadata().map(|meta| meta.name()),
            nest_rs_core::current_trace_id().map(|id| id.to_hex()),
        ));
        Ok(CallToolResult::success(vec![ContentBlock::text(
            "recorded",
        )]))
    }
}

#[tool_handler]
impl ServerHandler for SpanProbeTool {}

/// Mirrors the OTel interceptor's `http.request` span around the whole request.
fn under_span(inner: BoxEndpoint, span: tracing::Span) -> impl Endpoint {
    endpoint_fn(move |req| {
        let inner = inner.clone();
        let span = span.clone();
        async move { inner.call(req).instrument(span).await }
    })
}

#[tokio::test]
async fn a_tool_body_runs_under_its_own_operation_span_in_the_requests_trace() {
    // Thread-local is enough: on a current-thread runtime rmcp's spawn stays here.
    let logs = nest_rs_testing::LogCapture::install();

    let seen: SeenSpan = Arc::default();
    let host = SpanProbeTool { seen: seen.clone() };
    let app = under_span(
        endpoint(McpMount::public(), move || host.clone()),
        tracing::info_span!("http.request"),
    );

    call_tool(&TestClient::new(app), "/", "probe_span", None).await;

    let (span_name, trace_id) = seen
        .lock()
        .expect("probe lock")
        .clone()
        .expect("the tool ran and reported what it ran under");

    assert_eq!(
        span_name,
        Some("mcp.operation"),
        "an MCP operation is its own unit of work, not a second name for the \
         request that carried it — under rmcp's session mode one request carries \
         many, and filing them together makes \"what did this call do\" unanswerable",
    );
    let operations: Vec<_> = logs
        .spans()
        .into_iter()
        .filter(|span| span.target == "nest_rs::mcp" && span.name == "mcp.operation")
        .collect();
    // The handshake and the call are separate operations, so several spans.
    let spans: std::collections::HashSet<_> = operations
        .iter()
        .filter_map(|span| span.field("span_id"))
        .collect();
    assert_eq!(
        spans.len(),
        operations.len(),
        "no two operations share a span id: {operations:?}",
    );
    assert!(
        operations
            .iter()
            .all(|span| span.field("parent_span_id").is_some()),
        "each naming the request that carried it — the causal edge a flat id \
         could not express: {operations:?}",
    );

    let ran_under = operations
        .iter()
        .find(|span| span.field("trace_id").as_deref() == trace_id.as_deref())
        .unwrap_or_else(|| panic!("the tool's own trace has a span: {operations:?}"));
    assert_ne!(
        ran_under.field("span_id"),
        ran_under.field("parent_span_id"),
        "the operation is not a second name for the request that carried it",
    );
    assert_eq!(
        ran_under.field("mcp.method.name").as_deref(),
        Some("tools/call"),
        "{ran_under:?}",
    );
    assert_eq!(
        ran_under.field("mcp.operation.name").as_deref(),
        Some("probe_span"),
        "{ran_under:?}",
    );

    let served = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_mcp::unit::OPERATION.name(),
    );
    // `>=`: a notification files a line but opens no `mcp.operation` span.
    assert!(
        served.len() >= operations.len(),
        "every operation files a line: {} lines for {} operations: {served:?}",
        served.len(),
        operations.len(),
    );
    assert!(
        served.iter().all(|line| line.field("method").is_some()),
        "every line names the JSON-RPC method a client addressed: {served:?}",
    );
    // The protocol's method name, never rmcp's ident (`call_tool`).
    let tool_call = served
        .iter()
        .find(|line| line.field("method").as_deref() == Some("tools/call"))
        .unwrap_or_else(|| panic!("the tool call files a line: {served:?}"));
    assert_eq!(
        tool_call.field("operation").as_deref(),
        Some("probe_span"),
        "the line names the tool the request addressed: {served:?}",
    );
    // Absent, not empty, where the protocol addresses nothing.
    let handshake = served
        .iter()
        .find(|line| line.field("method").as_deref() == Some("initialize"))
        .unwrap_or_else(|| panic!("the handshake files a line: {served:?}"));
    assert_eq!(handshake.field("operation"), None, "{served:?}");
    assert!(
        served
            .iter()
            .all(|line| line.field("duration_ms").is_some()),
        "every line is timed: {served:?}",
    );
    // The ids are not asserted: the formatter reads them off the ambient context,
    // which `LogCapture` cannot see; `nest-rs-core`'s formatter tests cover them.
}
