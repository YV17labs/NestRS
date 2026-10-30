//! The MCP half of the same rule: a guard declaring that it checks MCP
//! operations writes the decision an operation runs, or it would pass every
//! tool it is bound beside.

use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::mcp::{McpError, McpView, mcp, tools};

#[injectable]
#[derive(Layer)]
struct RequestOnlyGuard;

#[async_trait]
impl Guard for RequestOnlyGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("refused at the request"))
    }
}

impl GuardFor<McpView> for RequestOnlyGuard {}

#[mcp]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[use_guards(RequestOnlyGuard)]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
