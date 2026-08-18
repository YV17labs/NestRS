//! Ambient request state reaches a tool body.
//!
//! rmcp dispatches each tool call on its own spawned task, so a task-local
//! installed around the poem endpoint does not reach it. `PropagatingHandler`
//! closes that gap: the endpoint stashes the scope in the request extensions,
//! rmcp forwards them as `http::request::Parts` into the operation's
//! `RequestContext`, and the handler re-installs the task-local *inside* the
//! spawned dispatch.
//!
//! This drives a real `tools/call` through the real endpoint. If it regresses,
//! `Scoped<T>` silently stops resolving and `Repo`-backed tools fall back to
//! failing closed — so the assertion is on what the tool body actually saw,
//! not on the transport succeeding.

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

/// A request-scoped provider the tool tries to resolve. Its presence is the
/// signal; nothing reads the payload.
struct Probe;

#[derive(Clone)]
struct ScopeProbeTool;

#[tool_router]
impl ScopeProbeTool {
    /// Reports over the wire whether `Scoped::from_context` found the
    /// task-local — the response *is* the assertion.
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

/// Mirrors the HTTP transport edge: run the inner endpoint under the ambient
/// request context so the MCP endpoint re-installs it across rmcp's spawn.
fn with_scope_extension(inner: impl IntoEndpoint) -> impl Endpoint {
    let container = Container::builder()
        .provide_scoped::<Probe, _>(|_| Probe)
        .build();
    let inner = Arc::new(inner.into_endpoint().map_to_response());
    poem::endpoint::make(move |req| {
        let inner = Arc::clone(&inner);
        let scope = Arc::new(RequestScope::new(container.clone()));
        let correlation = nest_rs_core::Correlation::mint();
        async move {
            nest_rs_core::with_request_scope(Some(scope), correlation, None, inner.call(req)).await
        }
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

/// The span identity a tool body ran under, reported back to the test.
/// What the tool body observed: the span it ran under, and the trace it was
/// filed in.
type SeenSpan = Arc<Mutex<Option<(Option<&'static str>, Option<String>)>>>;

#[derive(Clone)]
struct SpanProbeTool {
    seen: SeenSpan,
}

#[tool_router]
impl SpanProbeTool {
    /// Records the ambient span rather than asserting on it: the tool body is
    /// the only place that can answer what the dispatch installed.
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

/// Mirrors what the HTTP transport's outermost band does: run the whole request
/// under one span, the way the OTel interceptor's `http.request` wraps
/// everything below it.
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
    // A subscriber, so spans have identities at all. Thread-local is enough:
    // `#[tokio::test]` is a current-thread runtime, so rmcp's spawned dispatch
    // stays on this thread.
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

    // rmcp dispatches every operation on a spawned task, so without the ambient
    // being carried across it every event a tool and the services below it emit
    // is rooted at the spawn — attributable to no request at all.
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
    // The handshake and the call are separate operations, so several spans are
    // captured — which is itself the point: each is its own unit of work.
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

    // Each operation also files the family's line. It is the only place a tool
    // call reports itself: rmcp addresses many operations over one HTTP request,
    // so the endpoint's access line names the session and says nothing about the
    // work.
    let served = logs.find(nest_rs_core::operation_log::TARGET, "operation served");
    // At least one line per operation span, and possibly more: a **notification**
    // is dispatched work and files a line, but opens no `mcp.operation` span of
    // its own — it runs under the request's. So the counts are not equal, and
    // asserting they were would forbid the notification line rather than check it.
    assert!(
        served.len() >= operations.len(),
        "every operation files a line: {} lines for {} operations: {served:?}",
        served.len(),
        operations.len(),
    );
    assert!(
        served.iter().all(|line| line.field("operation").is_some()),
        "every line names which operation it was: {served:?}",
    );
    assert!(
        served
            .iter()
            .any(|line| line.field("operation").as_deref() == Some("call_tool")),
        "the line names the JSON-RPC method a client addressed: {served:?}",
    );
    assert!(
        served
            .iter()
            .all(|line| line.field("duration_ms").is_some()),
        "every line is timed: {served:?}",
    );
    // The ids are deliberately *not* asserted here, and the reason is worth
    // writing down: they are not fields of this event. The formatter reads them
    // off the ambient context at emission, so `LogCapture` — which records what
    // an event declared — cannot see them at any layer. That the line sits inside
    // a context at all is what `nest-rs-core`'s own formatter tests cover, and
    // this line emits through `RequestContinuation` precisely because the
    // dispatch installs its scope deeper than the line is written.
}
