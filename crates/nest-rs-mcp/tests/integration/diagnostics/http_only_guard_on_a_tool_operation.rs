//! The **operation** scope of the same rule, and the sibling of
//! `http_only_guard_on_a_tool`.
//!
//! `#[mcp]` and `#[tools]` are two decorators emitting the same `McpGuard`
//! bound at two scopes, and `framework.md` asks for "a trybuild snapshot per
//! edge, binding a guard that does not check it at that edge's site" — with
//! HTTP's three emitters as the worked example, "each underlin[ing] the
//! decorator the guard was written under, with a snapshot of its own". The host
//! scope had one and the operation scope did not, so deleting either half of
//! `#[tools]`' bound left the pair proved by the other's snapshot.

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard, async_trait};
use nest_rs_mcp::{McpError, mcp, tools};
use poem::Request;

#[injectable]
#[derive(Default)]
struct HttpOnlyGuard;

impl Layer for HttpOnlyGuard {}

#[async_trait]
impl Guard for HttpOnlyGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Ok(())
    }
}

#[mcp]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[use_guards(HttpOnlyGuard)]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

fn main() {}
