//! A description is not optional: the model reads it to choose between
//! operations, so an operation with neither a doc comment nor a stated
//! `description` is refused, at its name.

use nest_rs_mcp::{McpError, mcp, tools};

#[mcp(path = "/mcp/undescribed")]
#[derive(Clone, Default)]
struct UndescribedTool;

#[tools]
impl UndescribedTool {
    #[tool]
    #[public]
    async fn count(&self) -> Result<String, McpError> {
        Ok("0".to_owned())
    }
}

fn main() {}
