//! A `CallToolResult`'s content blocks are opaque to the entity round-trip: a
//! masked tool answering one is refused at its return type, with both
//! remedies, rather than failing every call at runtime.

use nest_rs::mcp::model::CallToolResult;
use nest_rs::mcp::{McpError, mcp, tools};

struct Read;
mod users {
    pub struct Entity;
}

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a user.")]
    #[authorize(Read, users::Entity)]
    async fn user(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(Vec::new()))
    }
}

fn main() {}
