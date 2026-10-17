//! `src/context.rs`: a registered `McpToolContext` captures off the admitted
//! request and installs what it captured around each operation, on the task
//! rmcp spawns.

use std::sync::Arc;

use nest_rs_core::{AmbientValue, injectable, module};
use nest_rs_http::Request;
use nest_rs_mcp::{
    AllowAllMcpGuard, BoxFuture, Captured, McpError, McpOperationGuard, McpToolContext,
    OperationOutcome, mcp, tools,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool_as;
use serde_json::json;

const PATH: &str = "/mcp/context";

struct Caller(String);

impl AmbientValue for Caller {}

/// Captures the caller the request names, as a data context captures its
/// executor and ability.
#[injectable]
#[derive(Default)]
struct CallerContext;

impl McpToolContext for CallerContext {
    fn capture(&self, req: &Request) -> Captured {
        Arc::new(req.header("x-caller").unwrap_or("nobody").to_owned())
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        match captured.downcast_ref::<String>() {
            Some(caller) => Box::pin(nest_rs_core::with_ambient(
                Arc::new(Caller(caller.clone())),
                inner,
            )),
            None => inner,
        }
    }
}

#[mcp(path = "/mcp/context")]
#[derive(Clone, Default)]
struct WhoamiTool;

#[tools]
impl WhoamiTool {
    /// Answer with the caller the tool context installed.
    #[tool]
    #[public]
    async fn whoami(&self) -> Result<String, McpError> {
        Ok(nest_rs_core::ambient::<Caller>()
            .map_or_else(|| "bare".to_owned(), |caller| caller.0.clone()))
    }
}

#[module(providers = [
    WhoamiTool,
    CallerContext as dyn McpToolContext,
    AllowAllMcpGuard as dyn McpOperationGuard,
])]
struct ContextModule;

#[tokio::test]
async fn a_tool_context_captures_off_the_request_and_installs_it_around_the_operation() {
    let app = TestApp::for_module::<ContextModule>()
        .await
        .expect("a tool context beside a tool boots");

    let body = call_tool_as(
        app.http(),
        PATH,
        "whoami",
        None,
        &[("x-caller", "ada")],
        json!({}),
    )
    .await;

    assert!(
        body.contains("ada") && !body.contains("bare"),
        "what `capture` read off the request reaches the tool: {body}",
    );
}
