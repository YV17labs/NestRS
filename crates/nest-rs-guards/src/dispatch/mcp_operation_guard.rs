//! [`GlobalPoolMcpGuard`] — the fallback `McpOperationGuard`.
//!
//! `/mcp` is `EdgePosture::Exempt` too. With no authz bridge registered, this
//! folds the global guard pool in-band, running `check_http`; a pooled
//! `check_mcp` runs in the per-operation chain. `/mcp` carries no
//! [`Public`](nest_rs_http::Public) marker, so a pooled `AuthnGuard` refuses an
//! unauthenticated call, and the builder seeds this only for a non-empty pool:
//! MCP's no-guard default is closed.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_mcp::{BoxFuture, McpOperationGuard};
use poem::{Request, Result};

use crate::denial::Denial;
use crate::dispatch::denial_convert::denial_to_http_error;
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

    /// The factory `use_guards_global` seeds as the MCP endpoint's fallback
    /// guard.
    pub fn factory(container: &Container) -> Arc<dyn McpOperationGuard> {
        Arc::new(Self::from_container(container))
    }
}

impl McpOperationGuard for GlobalPoolMcpGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // An empty resolved pool is not "every guard passed": a spec that
            // fails to resolve is dropped, so this guard stays closed on its own terms.
            if self.pool.is_empty() {
                tracing::warn!(
                    target: nest_rs_mcp::TARGET,
                    method = %req.method(),
                    path = %req.uri().path(),
                    reason = "global guard pool resolved empty",
                    "mcp operation denied",
                );
                return Err(denial_to_http_error(Denial::unauthorized(
                    "no guard resolved for this endpoint",
                )));
            }
            // `denial_to_http_error`: `Error::from_response` would drop the scope evidence.
            self.pool.check(req).await.map_err(denial_to_http_error)
        })
    }
}
