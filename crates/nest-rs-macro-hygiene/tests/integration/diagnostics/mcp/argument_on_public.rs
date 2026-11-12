//! `#[public]` alone is a posture, and `#[public(Action, Subject)]` names what
//! it reads. A lone word beside it — `read_only`, plausible as a flag — is neither,
//! so it is refused naming what the attribute takes, never dropped into an
//! ungated operation with the compiler silent.

use nest_rs::mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public(read_only)]
    async fn ping(&self) -> Result<String, nest_rs::mcp::McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
