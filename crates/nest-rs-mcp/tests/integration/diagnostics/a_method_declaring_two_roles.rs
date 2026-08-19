//! The MCP member of the one-role-per-method family, and the one that was
//! silent rather than merely worded differently: `#[tools]` took the **first**
//! role attribute and removed only that one, so the second survived on the
//! re-emitted method for rmcp to route as an operation nobody declared.

use nest_rs_mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Does the thing")]
    #[prompt(description = "Also a prompt?")]
    #[public]
    async fn run(&self) -> String {
        String::new()
    }
}

fn main() {}
