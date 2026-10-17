//! `#[use_pipes]` on an MCP tool is refused for good, naming the pipe bound
//! inside `Parameters<…>`, where the operation's arguments are decoded.

use nest_rs::mcp::tools;

struct Trim;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    #[use_pipes(Trim)]
    async fn ping(&self) -> Result<String, nest_rs::mcp::McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
