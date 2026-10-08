//! Ambient request state reaches a tool body across rmcp's spawned dispatch,
//! asserted on what the body saw rather than on the transport succeeding.

use std::sync::{Arc, Mutex};

use nest_rs_core::{Container, RequestScope};
use nest_rs_mcp::{
    AllowAllMcpGuard, CallToolResult, ContentBlock, McpError, McpMount, McpOperationGuard, Scoped,
    ServerHandler, endpoint, tool, tool_handler, tool_router,
};
use nest_rs_testing::mcp::call_tool;
use poem::test::TestClient;
use poem::{Endpoint, EndpointExt, IntoEndpoint};
use tracing::Instrument;

struct Probe;

#[derive(Clone)]
struct ScopeProbeTool;

#[tool_router]
impl ScopeProbeTool {
    #[tool(description = "Report whether the request scope reached this tool body.")]
    async fn probe_scope(&self) -> Result<CallToolResult, McpError> {
        let seen = Scoped::<Probe>::from_context().is_ok();
        Ok(CallToolResult::success(vec![ContentBlock::text(if seen {
            "scoped"
        } else {
            "unscoped"
        })]))
    }
}

#[tool_handler]
impl ServerHandler for ScopeProbeTool {}

/// Mirrors the HTTP transport edge, which installs the request scope.
fn with_scope_extension(inner: impl IntoEndpoint) -> impl Endpoint {
    let container = Container::builder()
        .provide_scoped::<Probe, _>(|_| Probe)
        .build();
    let inner = Arc::new(inner.into_endpoint().map_to_response());
    poem::endpoint::make(move |req| {
        let inner = Arc::clone(&inner);
        let scope = Arc::new(RequestScope::new(container.clone()));
        let correlation = nest_rs_core::Correlation::minted(None);
        async move { nest_rs_core::with_request_scope(Some(scope), correlation, inner.call(req)).await }
    })
}

#[tokio::test]
async fn ambient_request_scope_reaches_a_tool_body() {
    let guard = Arc::new(AllowAllMcpGuard) as Arc<dyn McpOperationGuard>;
    let app = with_scope_extension(endpoint(McpMount::deny_all().with_guard(guard), || {
        ScopeProbeTool
    }));
    let client = TestClient::new(app);

    let body = call_tool(&client, "/", "probe_scope", None).await;

    assert!(
        body.contains("scoped") && !body.contains("unscoped"),
        "the request scope must reach the tool body across rmcp's spawn — \
         `Scoped<T>` and every `Repo`-backed tool depend on it. Body: {body}",
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
fn under_span(inner: impl IntoEndpoint, span: tracing::Span) -> impl Endpoint {
    let inner = Arc::new(inner.into_endpoint().map_to_response());
    poem::endpoint::make(move |req| {
        let inner = Arc::clone(&inner);
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
    let guard = Arc::new(AllowAllMcpGuard) as Arc<dyn McpOperationGuard>;
    let app = under_span(
        endpoint(McpMount::deny_all().with_guard(guard), move || host.clone()),
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
