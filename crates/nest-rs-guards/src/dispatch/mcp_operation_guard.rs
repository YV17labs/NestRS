//! [`GlobalPoolMcpGuard`] — the fallback `McpOperationGuard`.
//!
//! The twin of [`GlobalPoolOperationGuard`](super::GlobalPoolOperationGuard):
//! `/mcp` is `EdgePosture::Exempt` too, so the per-operation seam is the only
//! gate. An app normally registers its authz bridge there (`AppMcpGuard as dyn
//! McpOperationGuard`); when it does not, this fallback folds the **global
//! guard pool** in-band, so `use_guards_global(...)` reaches `/mcp` exactly as
//! it reaches `/graphql` — a global `ThrottlerGuard` rate-limits tool calls,
//! and a forgotten bridge module still authenticates them.
//!
//! **Two deliberate differences from the GraphQL twin.** `/mcp` carries no
//! [`Public`](nest_rs_core::Public) marker, so a pooled `AuthnGuard` refuses an
//! unauthenticated tool call instead of admitting it to a resolver-level gate.
//! And `use_guards_global` seeds this fallback **only for a non-empty pool** —
//! MCP's no-guard default is closed, so an empty pool must leave `/mcp`
//! deny-all rather than fold an empty (i.e. pass-through) chain over it. That
//! gate lives in the builder, which is the one place that knows whether the
//! pool is empty; this guard therefore always has a chain to run.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_mcp::{BoxFuture, McpOperationGuard};
use poem::{Error, Request, Result};

use crate::denial::Denial;
use crate::dispatch::denial_convert::denial_to_http_response;
use crate::dispatch::global_pool::GlobalPoolChain;

/// Runs the global guard pool in-band per MCP operation — the fallback
/// [`McpOperationGuard`] when no app-specific bridge is registered, so `/mcp`
/// stays fail-secure *and* reachable by global guards under its `Exempt` edge
/// posture.
pub struct GlobalPoolMcpGuard {
    pool: GlobalPoolChain,
}

impl GlobalPoolMcpGuard {
    /// Resolve the global pool eagerly — the container is final at mount.
    pub fn from_container(container: &Container) -> Self {
        Self {
            pool: GlobalPoolChain::resolve(container, "POST /mcp (operation)"),
        }
    }

    /// The factory `use_guards_global` seeds as
    /// [`FallbackMcpGuard`](nest_rs_mcp::FallbackMcpGuard).
    pub fn factory(container: &Container) -> Arc<dyn McpOperationGuard> {
        Arc::new(Self::from_container(container))
    }
}

impl McpOperationGuard for GlobalPoolMcpGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // An empty pool must not read as "every guard passed": `/mcp`'s
            // default is closed, and the builder's non-empty-pool gate answers
            // a different question than this one (a spec that fails to resolve
            // is dropped from the chain). Deny here so this guard is closed on
            // its own terms, not by cooperation with the builder.
            if self.pool.is_empty() {
                tracing::warn!(
                    target: "nest_rs::mcp",
                    method = %req.method(),
                    path = %req.uri().path(),
                    reason = "global guard pool resolved empty",
                    "mcp operation denied",
                );
                return Err(Error::from_response(denial_to_http_response(
                    Denial::unauthorized("no guard resolved for this endpoint"),
                )));
            }
            self.pool
                .check(req)
                .await
                .map_err(|denial| Error::from_response(denial_to_http_response(denial)))
        })
    }
}
