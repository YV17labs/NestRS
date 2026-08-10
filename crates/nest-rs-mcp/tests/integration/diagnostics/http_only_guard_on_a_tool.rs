//! The MCP half of the same rule. Both scopes are checked — the host struct's
//! guards and the operation's own fold into one per-operation chain, so both run
//! `check_mcp` and both owe the marker.

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard, async_trait};
use nest_rs_mcp::{McpError, mcp, tools};
use poem::Request;

#[injectable]
#[derive(Default)]
struct HttpOnlyGuard;

impl Layer for HttpOnlyGuard {}

#[async_trait]
impl Guard for HttpOnlyGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Ok(())
    }
}

#[mcp]
#[use_guards(HttpOnlyGuard)]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
