//! The MCP member of the host-scope-layer family — one sentence, four edges.
//! This edge worded it itself and named three siblings; the list was a roll
//! call that every edge added later would have inherited wrong.
//!
//! Only the impl half is decorated, for the reason `tool_without_posture`
//! gives: pairing it with `#[mcp]` cascades a second error about the
//! `ServerHandler` the refused expansion never wrote.

use nest_rs_mcp::tools;

struct AllowAll;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
#[use_guards(AllowAll)]
impl DemoTool {
    #[tool(description = "Does the thing")]
    #[public]
    async fn run(&self) -> String {
        String::new()
    }
}

fn main() {}
