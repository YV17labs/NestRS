//! A resource template's description is what a client and its model read to
//! choose it: one with neither a doc comment nor a stated `description` is
//! refused, at its name, as a tool's is.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }
}

fn main() {}
