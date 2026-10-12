//! Two `#[meta]` values of one type on one tool: refused at the second.

use nest_rs::mcp::{McpError, mcp, tools};

struct Limit(u32);

#[mcp]
#[derive(Clone, Default)]
struct PingTools;

#[tools]
impl PingTools {
    #[tool(description = "Answer pong.")]
    #[public]
    #[meta(Limit(10))]
    #[meta(Limit(20))]
    async fn ping(&self) -> Result<String, McpError> {
        Ok(String::new())
    }
}

fn main() {}
