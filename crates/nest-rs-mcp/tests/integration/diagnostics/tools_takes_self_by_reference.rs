//! A `#[tools]` method borrows its host. One with no receiver is refused with
//! that fact, rather than a wrapper that calls it with `self` it does not take.

use nest_rs_mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Does the thing")]
    #[public]
    async fn run() -> Result<String, nest_rs_mcp::McpError> {
        Ok(String::new())
    }
}

fn main() {}
