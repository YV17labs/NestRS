//! `resources/list` sends no argument, so a listing takes none but a
//! request-scoped provider.

use nest_rs::mcp::model::Resource;
use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    #[resources]
    #[public]
    async fn notes(&self, page: u32) -> Result<Vec<Resource>, McpError> {
        let _ = page;
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
