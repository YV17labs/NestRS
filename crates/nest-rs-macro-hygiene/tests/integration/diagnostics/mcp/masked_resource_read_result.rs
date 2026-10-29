//! A `ReadResourceResult` is opaque to the entity round-trip, as a
//! `CallToolResult` is: a masked resource answering one is refused at its
//! return type, with both remedies.

use nest_rs::mcp::model::ReadResourceResult;
use nest_rs::mcp::{McpError, mcp, tools};

struct Read;
mod users {
    pub struct Entity;
}

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    /// One user's avatar.
    #[resource(template = "avatar://{id}")]
    #[authorize(Read, users::Entity)]
    async fn avatar(&self, id: u32) -> Result<ReadResourceResult, McpError> {
        let _ = id;
        Ok(ReadResourceResult::new(Vec::new()))
    }
}

fn main() {}
