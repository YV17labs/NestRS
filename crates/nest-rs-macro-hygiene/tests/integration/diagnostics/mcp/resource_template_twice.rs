//! A host offers a URI to its templates in order, so the second of two equal
//! templates would never read one.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// One note.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }

    /// The same note, again.
    #[resource(template = "note://{id}")]
    #[public]
    async fn again(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }
}

fn main() {}
