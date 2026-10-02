//! An MCP operation answers a `Result` an `McpError` converts into: its guard
//! chain, its access posture and its pipes refuse by returning. The check is by
//! type, at the one site the wrapper refuses, so it is said once, at the return
//! type — a `Result` renamed on import is accepted, which a spelling could not
//! tell.

use nest_rs_mcp::{mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    async fn ping(&self) -> String {
        "ok".into()
    }
}

fn main() {}
