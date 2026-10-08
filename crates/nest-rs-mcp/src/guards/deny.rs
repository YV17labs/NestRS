//! Default deny-all guard for MCP endpoints mounted without an explicit
//! [`McpOperationGuard`].

use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Error, Request, Response};

use crate::guard::McpOperationGuard;

pub(crate) struct DenyAllMcpGuard;

/// The fail-closed posture, said once at boot.
pub(crate) fn deny_all() -> Arc<dyn McpOperationGuard> {
    tracing::warn!(
        target: crate::TARGET,
        mode = "deny_all",
        "no operation guard registered — mcp endpoint is deny-all",
    );
    Arc::new(DenyAllMcpGuard)
}

impl McpOperationGuard for DenyAllMcpGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> crate::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move {
            tracing::warn!(
                target: crate::TARGET,
                method = %req.method(),
                path = %req.uri().path(),
                reason = "no McpOperationGuard registered",
                "mcp operation denied",
            );
            Err(Error::from_response(
                Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body("unauthorized"),
            ))
        })
    }
}
