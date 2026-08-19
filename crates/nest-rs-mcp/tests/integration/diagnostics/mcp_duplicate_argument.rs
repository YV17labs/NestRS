//! An `#[mcp]` key written twice is refused rather than resolved by source
//! order.
//!
//! The dropped declaration here is the **path**, which is a join key: it decides
//! which peers share the host's endpoint, and therefore which surface a client
//! pointed at one URL is told it reached.

use nest_rs_mcp::{mcp, tools};

#[mcp(path = "/a", path = "/b")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("ok".into())
    }
}

fn main() {}
