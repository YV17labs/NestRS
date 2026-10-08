//! Explicit allow-all guard for MCP endpoints intentionally served without
//! authentication.

use nest_rs_core::injectable;
use poem::Request;

use crate::guard::McpOperationGuard;

/// Admits every request unchanged, for a deliberately public tool surface wired
/// as `dyn McpOperationGuard`; without an [`McpOperationGuard`] an endpoint is deny-all.
#[injectable]
#[derive(Default)]
pub struct AllowAllMcpGuard;

impl McpOperationGuard for AllowAllMcpGuard {
    fn before<'a>(&'a self, _req: &'a mut Request) -> crate::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}
