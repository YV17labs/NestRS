//! [`GlobalPoolMcpGuard`] — the global guard pool, run in band on `/mcp`.
//!
//! `/mcp` gates in band too. Whenever the pool is non-empty this runs its
//! `check_http` at the request, with a registered authz bridge run inside it; a
//! pooled `check` runs in the per-operation chain. `/mcp` is `Gated`, so a
//! pooled `AuthnGuard` refuses an unauthenticated call, and the endpoint runs
//! this only for a non-empty pool: MCP's no-guard default is closed.

use std::sync::Arc;

use nest_rs_core::__private::ResolvedLayer;
use nest_rs_core::Container;
use nest_rs_guards::__private::{resolve_global_chain, split_at_covered};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::poem_bridge::error_from_poem;
use nest_rs_http::__private::{check_request_chain, denial_to_http_error};
use nest_rs_http::{HttpError, Request, Result};

use crate::context::{Captured, OperationOutcome};
use crate::endpoint::endpoint_site;
use crate::guard::{BoxFuture, McpOperationGuard};

/// Runs the global guard pool in band per MCP request, the registered
/// [`McpOperationGuard`] (the authz bridge) inside it, so `/mcp` stays
/// fail-secure *and* reachable by every global guard.
pub(crate) struct GlobalPoolMcpGuard {
    /// The pool's guards listed before the first one the bridge covers.
    head: Vec<ResolvedLayer<dyn Guard>>,
    bridge: Option<Arc<dyn McpOperationGuard>>,
    /// The rest of the pool, minus what the bridge covers.
    tail: Vec<ResolvedLayer<dyn Guard>>,
    /// No pooled guard resolved, so none decided anything.
    unresolved: bool,
}

impl GlobalPoolMcpGuard {
    /// Resolve the global pool eagerly — the container is final at mount — with
    /// `bridge` run where the pool lists the first guard it
    /// [covers](McpOperationGuard::covers), else after the pool.
    pub(crate) fn new(container: &Container, bridge: Option<Arc<dyn McpOperationGuard>>) -> Self {
        let pool = resolve_global_chain(container, "POST /mcp (operation)");
        let unresolved = pool.is_empty();
        let covers = bridge.as_deref().map_or(&[][..], |bridge| bridge.covers());
        let (head, tail) = split_at_covered(pool, covers);
        Self {
            head,
            bridge,
            tail,
            unresolved,
        }
    }
}

/// A pooled guard's refusal, as the endpoint answers it.
fn denied(denial: Denial) -> HttpError {
    error_from_poem(denial_to_http_error(denial))
}

impl McpOperationGuard for GlobalPoolMcpGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // An empty resolved pool is not "every guard passed": a spec that
            // fails to resolve is dropped, so this guard stays closed on its own terms.
            if self.unresolved {
                tracing::warn!(
                    target: crate::TARGET,
                    method = %req.method(),
                    path = %req.uri().path(),
                    reason = "global guard pool resolved empty",
                    "mcp operation denied",
                );
                return Err(denied(Denial::unauthorized(
                    "no guard resolved for this endpoint",
                )));
            }
            // `denial_to_http_error`: `Error::from_response` would drop the scope
            // evidence. The pool's refusal keeps its status: a pooled
            // throttler's `429` stays a `429`.
            let site = endpoint_site(req);
            check_request_chain(site, &self.head, req)
                .await
                .map_err(denied)?;
            if let Some(bridge) = &self.bridge {
                bridge.before(req).await?;
            }
            check_request_chain(site, &self.tail, req)
                .await
                .map_err(denied)
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
