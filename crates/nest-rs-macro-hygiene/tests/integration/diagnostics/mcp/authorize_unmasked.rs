//! `unmasked` is refused **by name** on every edge, with the fact that makes it
//! meaningless: no reply mask is left to switch off, since each wire row a reply
//! carries was judged for its caller where `#[expose]`'s conversion built it.

use nest_rs::mcp::tools;

struct Read;
mod users {
    pub struct Entity;
}

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[authorize(Read, users::Entity, unmasked)]
    async fn ping(&self) -> Result<String, nest_rs::mcp::McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
