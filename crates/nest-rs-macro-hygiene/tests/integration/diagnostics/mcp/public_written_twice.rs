//! A decorator read once is refused by name when written twice. The first copy
//! was taken and the second left on the method, where rustc reported
//! `cannot find attribute` — no decorator, no reason, no remedy. One sentence
//! for every such attribute (`nest_rs::codegen::take_single_attr`), pinned at
//! each edge.
//!
//! Only the impl half is decorated, for the reason `tool_without_posture`
//! records.

use nest_rs::mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs::mcp::McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
