//! A stated description that trims to nothing is no description, and is refused
//! at the literal with the sentence a missing one gets.

use nest_rs_mcp::{McpError, mcp, tools};

#[mcp(path = "/mcp/blank")]
#[derive(Clone, Default)]
struct BlankTool;

#[tools]
impl BlankTool {
    #[tool(description = "   ")]
    #[public]
    async fn count(&self) -> Result<String, McpError> {
        Ok("0".to_owned())
    }
}

fn main() {}
