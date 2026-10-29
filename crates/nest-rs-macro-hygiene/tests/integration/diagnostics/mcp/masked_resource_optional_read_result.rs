//! A `ReadResourceResult` is as opaque inside the `Option` a resource answers
//! as it is bare: the mask would reach it, so a masked resource answering one
//! is refused at its return type, with both remedies.

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
    /// One user's avatar, when the caller may read it.
    #[resource(template = "avatar://{id}")]
    #[authorize(Read, users::Entity)]
    async fn avatar(&self, id: u32) -> Result<Option<ReadResourceResult>, McpError> {
        let _ = id;
        Ok(None)
    }
}

fn main() {}
