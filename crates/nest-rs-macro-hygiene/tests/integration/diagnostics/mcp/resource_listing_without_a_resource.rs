//! A listing names URIs a client then reads: one beside no `#[resource]` lists
//! what nothing reads.

use nest_rs::mcp::model::Resource;
use nest_rs::mcp::{McpError, mcp, tools};

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoHost;

#[tools]
impl DemoHost {
    #[resources]
    #[public]
    async fn notes(&self) -> Result<Vec<Resource>, McpError> {
        Ok(Vec::new())
    }
}

fn main() {}
