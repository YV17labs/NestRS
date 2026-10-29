//! Each variable of a template is an argument of the method that reads it: one
//! no parameter takes would be read off the URI and dropped.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// One note.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self) -> Result<String, McpError> {
        Ok("note".to_owned())
    }
}

fn main() {}
