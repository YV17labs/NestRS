//! Two tools whose `#[cfg]`s both hold, one named after the other's method, are
//! two routes for one name — refused by rustc, which is what evaluates the
//! conditions.

use nest_rs_mcp::model::GetPromptResult;
use nest_rs_mcp::{McpError, mcp, tools};

#[mcp]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[cfg(all())]
    #[tool(description = "Look one up.")]
    #[public]
    async fn lookup(&self) -> Result<String, McpError> {
        Ok("first".into())
    }

    #[tool(name = "lookup", description = "Look one up, again.")]
    #[public]
    async fn second(&self) -> Result<String, McpError> {
        Ok("second".into())
    }

    #[cfg(all())]
    #[prompt(name = "brief", description = "A brief.")]
    #[public]
    async fn brief(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(Vec::new()))
    }

    #[cfg(all())]
    #[prompt(name = "brief", description = "Another brief.")]
    #[public]
    async fn brief_again(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(Vec::new()))
    }
}

fn main() {}
