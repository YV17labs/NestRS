//! Default deny-all guard for MCP endpoints mounted without an explicit
//! [`McpOperationGuard`](crate::guard::McpOperationGuard).

use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Error, Request, Response};

use crate::guard::McpOperationGuard;

pub(crate) struct DenyAllMcpGuard;

/// The fail-closed posture, said once at boot. Both mount paths — an explicit
/// `endpoint_with_guard(None, ..)` and
/// [`resolve_operation_guard`](crate::resolve_operation_guard) finding neither
/// a registered guard nor a global pool — land here, so the endpoint can never
/// open silently.
pub(crate) fn deny_all() -> Arc<dyn McpOperationGuard> {
    // Mirrors GraphQL's unguarded-schema warning so a deny-all endpoint born of
    // a missing guard import is never silent.
    tracing::warn!(
        target: "nest_rs::mcp",
        mode = "deny_all",
        "no operation guard registered — mcp endpoint is deny-all",
    );
    Arc::new(DenyAllMcpGuard)
}

impl McpOperationGuard for DenyAllMcpGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> crate::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move {
            // Fail-closed default: no `McpOperationGuard` was registered, so
            // every operation is denied. Log it loudly — this is a security
            // misconfiguration, not a routine denial, and is exactly the event
            // queried under incident. Fields carry the request coordinates; the
            // principal is unknown (that is the misconfiguration).
            tracing::warn!(
                target: "nest_rs::mcp",
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
