//! A `#[resource]` is read at the URIs its template writes, so one with no
//! `template` names nothing a client can read.

use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// The one note.
    #[resource]
    #[public]
    async fn note(&self) -> Result<String, McpError> {
        Ok("note".to_owned())
    }
}

fn main() {}
