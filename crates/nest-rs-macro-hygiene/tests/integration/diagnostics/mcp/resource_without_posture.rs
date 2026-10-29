//! A `#[resource]` declares its access posture like a `#[tool]`: a read the
//! developer forgot to think about never reaches a model ungated and unmasked.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    #[resource(template = "note://{id}", description = "One note.")]
    async fn note(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }
}

fn main() {}
