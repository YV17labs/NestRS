//! `resources/list` is answered by one method per host: a second listing would
//! never run.

use nest_rs::mcp::model::Resource;
use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    #[resources]
    #[public]
    async fn notes(&self) -> Result<Vec<Resource>, McpError> {
        Ok(Vec::new())
    }

    #[resources]
    #[public]
    async fn drafts(&self) -> Result<Vec<Resource>, McpError> {
        Ok(Vec::new())
    }

    /// One note.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }
}

fn main() {}
