//! `#[public]` is a flag: its presence *is* the declaration, so an argument on
//! it is a compile error rather than something the expansion drops.
//!
//! One of the four sites the shared refusal covers — `nest_rs_codegen::take_flag_attr`
//! words it once, and the flag is `#[public]` at every edge because that is the
//! greppable authn/authz site `CLAUDE.md` reserves. Before the refusal,
//! `#[public(read_only)]` — plausible beside `#[authorize(Action, Entity)]`,
//! which does take arguments — shipped an ungated, unmasked operation with the
//! compiler silent.
//!
//! Only the impl half is decorated, for the reason `tool_without_posture`
//! records: pairing it with `#[mcp]` cascades a second error about the
//! `ServerHandler` the refused expansion never wrote.

use nest_rs_mcp::tools;

#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[public(read_only)]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
