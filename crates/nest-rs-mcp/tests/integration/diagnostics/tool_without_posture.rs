//! A `#[tool]` with neither `#[authorize(...)]` nor `#[public]` must not
//! compile — an operation the developer forgot to think about never ships
//! ungated and unmasked to a language model.
//!
//! Only the impl half is decorated: `#[tools]` is what refuses, and pairing it
//! with `#[mcp]` here would cascade a second error about the `ServerHandler`
//! the refused expansion never wrote — pinning rmcp's internals in a snapshot
//! that exists to pin one sentence of ours.

use nest_rs_mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
