//! `#[mcp]` declares the host and its endpoint, so it names the struct only.
//! On the impl block it must point at `#[tools]`.
//!
//! The host struct here is left bare on purpose: a decorated one would also
//! fail `McpHost` (no `ServerHandler` without a `#[tools]` block) and bury the
//! diagnostic under a trait-bound cascade.

use nest_rs_mcp::mcp;

struct DemoTool;

#[mcp]
impl DemoTool {
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("pong".to_string())
    }
}

fn main() {}
