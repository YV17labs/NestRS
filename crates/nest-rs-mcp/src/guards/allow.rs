//! Explicit allow-all guard for MCP endpoints intentionally served without
//! authentication.

use nest_rs_core::injectable;
use poem::Request;

use crate::guard::McpOperationGuard;

/// Opt-in counterpart to the default deny-all. An MCP endpoint mounted without
/// an [`McpOperationGuard`] fails **closed** (deny-all) — so a genuinely public
/// tool must declare that intent by wiring this guard as `dyn
/// McpOperationGuard`. It admits every request unchanged; authorization, if
/// any, is left to the tool. Reach for it only when the tool surface is
/// deliberately public (no claims, no row-level data) — never to silence the
/// deny-all on an endpoint that should authenticate.
#[injectable]
#[derive(Default)]
pub struct AllowAllMcpGuard;

impl McpOperationGuard for AllowAllMcpGuard {
    fn before<'a>(&'a self, _req: &'a mut Request) -> crate::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}
