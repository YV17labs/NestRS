//! A host routes each tool name to one method. Two methods declaring one `name`
//! are refused, rather than the later replacing the earlier in the router.

use nest_rs_mcp::{McpError, mcp, tools};

#[mcp]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(name = "lookup", description = "Look one up.")]
    #[public]
    async fn first(&self) -> Result<String, McpError> {
        Ok("first".into())
    }

    #[tool(name = "lookup", description = "Look one up, again.")]
    #[public]
    async fn second(&self) -> Result<String, McpError> {
        Ok("second".into())
    }
}

fn main() {}
