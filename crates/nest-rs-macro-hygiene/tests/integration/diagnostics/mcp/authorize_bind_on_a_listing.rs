//! `bind = Service` reads the one id a `#[resource]`'s template names; a listing
//! names none, and refuses it by name.

use nest_rs::mcp::model::Resource;
use nest_rs::mcp::{McpError, mcp, tools};

struct Read;
struct DemoService;

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    /// One row.
    #[resource(template = "demo://{id}")]
    #[public]
    async fn one(&self, id: u32) -> Result<String, McpError> {
        Ok(id.to_string())
    }

    #[resources]
    #[authorize(Read, bind = DemoService)]
    async fn all(&self) -> Result<Vec<Resource>, McpError> {
        Ok(Vec::new())
    }
}

fn main() {}
