//! A description only the compiler can read — a doc line written as a macro, a
//! stated macro or constant — is checked by the compiler: one that evaluates
//! blank fails the build with the sentence a missing description gets, at the
//! operation's name. An empty `include_str!` shipped to the model as the whole
//! description while `#[doc = "   "]` was refused.

use nest_rs::mcp::model::GetPromptResult;
use nest_rs::mcp::{McpError, mcp, tools};

const NOTHING: &str = " \t";

#[mcp(path = "/mcp/evaluated")]
#[derive(Clone, Default)]
struct EvaluatedTool;

#[tools]
impl EvaluatedTool {
    #[doc = concat!("")]
    #[tool]
    #[public]
    async fn from_the_doc(&self) -> Result<String, McpError> {
        Ok("0".to_owned())
    }

    #[tool(description = concat!(" ", "\u{3000}"))]
    #[public]
    async fn from_a_stated_macro(&self) -> Result<String, McpError> {
        Ok("0".to_owned())
    }

    #[tool(description = NOTHING)]
    #[public]
    async fn from_a_stated_constant(&self) -> Result<String, McpError> {
        Ok("0".to_owned())
    }

    #[doc = concat!(" ")]
    #[prompt]
    #[public]
    async fn a_prompt(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(Vec::new()))
    }
}

fn main() {}
