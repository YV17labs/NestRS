//! What a resource read answers is read by its type: a value that is no
//! resource's contents is refused once, at the return type, naming the
//! shapes that are.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// One note's length.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self, id: u32) -> Result<u32, McpError> {
        Ok(id)
    }
}

fn main() {}
