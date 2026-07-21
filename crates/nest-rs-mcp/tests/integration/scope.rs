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

use std::sync::Arc;

use nest_rs_core::{Container, RequestScope};
use nest_rs_mcp::{
    AllowAllMcpGuard, CallToolResult, ContentBlock, McpError, McpOperationGuard, Scoped,
    ServerHandler, endpoint_with_guard, tool, tool_handler, tool_router,
};
use nest_rs_testing::mcp::call_tool;
use poem::test::TestClient;
use poem::{Endpoint, EndpointExt, IntoEndpoint, Request};

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

/// Mirrors `RequestScopeEndpoint`: put an `Arc<RequestScope>` in the request
/// extensions so the MCP endpoint installs it as the task-local.
fn with_scope_extension(inner: impl IntoEndpoint) -> impl Endpoint {
    let container = Container::builder()
        .provide_scoped::<Probe, _>(|_| Probe)
        .build();
    inner.into_endpoint().before(move |mut req: Request| {
        let scope = Arc::new(RequestScope::new(container.clone()));
        req.extensions_mut().insert(scope);
        async move { Ok(req) }
    })
}

#[tokio::test]
async fn ambient_request_scope_reaches_a_tool_body() {
    let guard = Arc::new(AllowAllMcpGuard) as Arc<dyn McpOperationGuard>;
    let app = with_scope_extension(endpoint_with_guard(guard, None, || ScopeProbeTool));
    let client = TestClient::new(app);

    let body = call_tool(&client, "/", "probe_scope", None).await;

    assert!(
        body.contains("scoped") && !body.contains("unscoped"),
        "the request scope must reach the tool body across rmcp's spawn — \
         `Scoped<T>` and every `Repo`-backed tool depend on it. Body: {body}",
    );
}
