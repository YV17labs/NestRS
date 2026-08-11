//! `#[use_interceptors]` is not bridged on MCP, so binding one is a named
//! compile error rather than a layer that silently never runs — the defect
//! `reject_http_only_layers` exists to prevent, pinned on the fifth edge as it
//! already is on GraphQL and WS.
//!
//! Impl half only, for the reason spelled out in `tool_without_posture`.

use nest_rs_mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    #[use_interceptors(SomeInterceptor)]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
