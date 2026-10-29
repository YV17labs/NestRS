//! A listing's entries are URIs and names the entity round-trip cannot read, so
//! a gated `#[resources]` says it is unmasked rather than being masked as a
//! no-op.

use nest_rs::mcp::model::Resource;
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
    #[resources]
    #[authorize(Read, users::Entity)]
    async fn users(&self) -> Result<Vec<Resource>, McpError> {
        Ok(Vec::new())
    }

    /// One user.
    #[resource(template = "user://{id}")]
    #[public]
    async fn user(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }
}

fn main() {}
