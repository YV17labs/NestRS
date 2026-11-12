//! One role written again is refused counting every copy the method carries,
//! rather than naming the first two attributes as though they were two roles.

use nest_rs::mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Does the thing")]
    #[tool(description = "Does the thing again")]
    #[tool(description = "And once more")]
    #[public]
    async fn run(&self) -> String {
        String::new()
    }
}

fn main() {}
