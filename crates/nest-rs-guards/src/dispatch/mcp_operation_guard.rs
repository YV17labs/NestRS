//! [`GlobalPoolMcpGuard`] — the global guard pool, run in band on `/mcp`.
//!
//! `/mcp` is `EdgePosture::Exempt` too. Whenever the pool is non-empty this
//! runs its `check_http` at the request, with a registered authz bridge run
//! inside it; a pooled `check_mcp` runs in the per-operation chain. `/mcp`
//! carries no `Posture::InBand` (its requests read `Gated`), so a pooled
//! `AuthnGuard` refuses an unauthenticated call, and the builder seeds this
//! only for a non-empty pool: MCP's no-guard default is closed.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_mcp::{BoxFuture, Captured, McpOperationGuard, OperationOutcome};
use poem::{Request, Result};

use crate::denial::Denial;
use crate::dispatch::denial_convert::denial_to_http_error;
use crate::dispatch::global_pool::GlobalPoolChain;

/// Runs the global guard pool in band per MCP request, the registered
/// [`McpOperationGuard`] (the authz bridge) inside it, so `/mcp` stays
/// fail-secure *and* reachable by every global guard under its `Exempt` edge
/// posture.
pub struct GlobalPoolMcpGuard {
    /// The pool's guards listed before the first one the bridge covers.
    head: GlobalPoolChain,
    bridge: Option<Arc<dyn McpOperationGuard>>,
    /// The rest of the pool, minus what the bridge covers.
    tail: GlobalPoolChain,
    /// No pooled guard resolved, so none decided anything.
    unresolved: bool,
}

impl GlobalPoolMcpGuard {
    /// Resolve the global pool eagerly — the container is final at mount — with
    /// `bridge` run where the pool lists the first guard it
    /// [covers](McpOperationGuard::covers), else after the pool.
    pub fn from_container(
        container: &Container,
        bridge: Option<Arc<dyn McpOperationGuard>>,
    ) -> Self {
        let pool = GlobalPoolChain::resolve(container, "POST /mcp (operation)");
        let unresolved = pool.is_empty();
        let covers = bridge.as_deref().map_or(&[][..], |bridge| bridge.covers());
        let (head, tail) = pool.split_at_covered(covers);
        Self {
            head,
            bridge,
            tail,
            unresolved,
        }
    }

    /// The factory `use_guards_global` seeds as the MCP endpoint's global pool
    /// runner.
    pub fn factory(
        container: &Container,
        bridge: Option<Arc<dyn McpOperationGuard>>,
    ) -> Arc<dyn McpOperationGuard> {
        Arc::new(Self::from_container(container, bridge))
    }
}

impl McpOperationGuard for GlobalPoolMcpGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // An empty resolved pool is not "every guard passed": a spec that
            // fails to resolve is dropped, so this guard stays closed on its own terms.
            if self.unresolved {
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
            self.head.check(req).await.map_err(denial_to_http_error)?;
            if let Some(bridge) = &self.bridge {
                bridge.before(req).await?;
            }
            self.tail.check(req).await.map_err(denial_to_http_error)
        })
    }

    fn capture(&self, req: &Request) -> Option<Captured> {
        self.bridge.as_ref()?.capture(req)
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        match &self.bridge {
            Some(bridge) => bridge.around(captured, inner),
            None => inner,
        }
    }
}
