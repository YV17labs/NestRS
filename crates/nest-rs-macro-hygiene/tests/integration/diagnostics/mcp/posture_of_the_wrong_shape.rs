//! A posture that is not `#[word(Action, Subject)]` is refused in the grammar's
//! sentence, whatever stops the parse. syn's `expected parentheses` and
//! `expected ,` name neither the form nor what it takes.

use nest_rs::mcp::{McpError, mcp, tools};

struct Read;

mod posts {
    pub struct Entity;
}

#[mcp(path = "/mcp/value")]
#[derive(Clone, Default)]
struct ValueOnPublic;

#[tools]
impl ValueOnPublic {
    #[tool(description = "List.")]
    #[public = "x"]
    async fn list(&self) -> Result<String, McpError> {
        Ok(String::new())
    }
}

#[mcp(path = "/mcp/comma")]
#[derive(Clone, Default)]
struct NoCommaOnPublic;

#[tools]
impl NoCommaOnPublic {
    #[tool(description = "List.")]
    #[public(Read posts::Entity)]
    async fn list(&self) -> Result<String, McpError> {
        Ok(String::new())
    }
}

#[mcp(path = "/mcp/bare")]
#[derive(Clone, Default)]
struct BareAuthorize;

#[tools]
impl BareAuthorize {
    #[tool(description = "List.")]
    #[authorize]
    async fn list(&self) -> Result<String, McpError> {
        Ok(String::new())
    }
}

fn main() {}
