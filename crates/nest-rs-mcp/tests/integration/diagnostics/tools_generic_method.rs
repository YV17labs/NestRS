//! A tool is called with the arguments a client sends and nothing else, so a
//! type parameter has nothing to be inferred from.

use nest_rs_mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Does the thing")]
    #[public]
    async fn run<T: Default + ToString>(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok(T::default().to_string())
    }
}

fn main() {}
