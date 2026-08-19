//! `bind = Service` is refused by name here too — the second of the two sites
//! that shared the sentence and pinned it nowhere.

use nest_rs_mcp::{mcp, tools};

struct Update;
struct DemoService;
mod demo {
    pub struct Entity;
}

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[authorize(Update, bind = DemoService)]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("ok".into())
    }
}

fn main() {}
