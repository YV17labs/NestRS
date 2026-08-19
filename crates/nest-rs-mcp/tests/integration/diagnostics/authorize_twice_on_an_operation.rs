//! Two `#[authorize(...)]` on one operation is refused, not resolved.
//!
//! MCP takes the shared `PostureRules` verbatim, so this pins the third site of
//! one sentence — and the third of `nest_rs_codegen::at_most_one_authorize`'s
//! call sites, which had no snapshot anywhere.

use nest_rs_mcp::{mcp, tools};

struct Read;
struct Update;
mod users {
    pub struct Entity;
}

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[authorize(Read, users::Entity)]
    #[authorize(Update, users::Entity)]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("ok".into())
    }
}

fn main() {}
