//! A template variable is parsed off the URI with `FromStr`; `Parameters<…>`
//! carries a tool's JSON arguments, which a URI does not send, so a parameter
//! wrapped in it is refused by what it is, not as a name matching nothing.

use nest_rs::mcp::{McpError, Parameters, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// One note.
    #[resource(template = "note://{id}")]
    #[public]
    async fn note(&self, id: Parameters<u32>) -> Result<String, McpError> {
        let Parameters(id) = id;
        Ok(id.to_string())
    }
}

fn main() {}
