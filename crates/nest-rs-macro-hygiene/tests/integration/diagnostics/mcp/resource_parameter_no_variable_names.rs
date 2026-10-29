//! A resource reads its arguments off its URI: a parameter no variable of the
//! template names has nothing to fill it.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// One note.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self, id: u32, name: String) -> Result<String, McpError> {
        Ok(format!("{id} {name}"))
    }
}

fn main() {}
