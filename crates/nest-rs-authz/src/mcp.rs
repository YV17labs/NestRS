//! MCP surface for [`nest_rs_authz`](crate). Enabled by the `mcp` Cargo feature.
//!
//! Authenticate MCP HTTP requests with the same guard chain controllers use,
//! then install the caller's ambient [`Ability`] for the operation's duration.
//!
//! Field-level masking inside a tool body is [`crate::masked_output_ambient`] —
//! MCP tool output is arbitrary JSON-RPC content, so it cannot be masked
//! transparently the way the HTTP route shaper does.

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_guards::{Guard, denial_to_http_response};
use nest_rs_mcp::{BoxFuture, Captured, McpOperationGuard, OperationOutcome};
use poem::{Error, Request, Result};

use crate::{Ability, run_ability_chain, with_ability};

/// Runs `A` then `G` on each MCP HTTP request and scopes the operation to the
/// resulting ability. Inject it as `dyn McpOperationGuard`.
#[injectable]
pub struct McpAbilityBridge<A: Guard, G: Guard> {
    #[inject]
    auth: Arc<A>,
    #[inject]
    ability: Arc<G>,
}

impl<A: Guard, G: Guard> McpOperationGuard for McpAbilityBridge<A, G> {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Same ordering as the GraphQL bridge, from the same function; only
            // the error shape differs. Mapping the `Denial` (rather than
            // answering a blanket `401`) is what lets a `429` from a throttler
            // in the chain reach the client as a `429`.
            run_ability_chain(&*self.auth, &*self.ability, req)
                .await
                .map_err(|denial| Error::from_response(denial_to_http_response(denial)))
        })
    }

    /// The caller's ability, read from the request the chain just authorized.
    /// Absent (anonymous) ⇒ nothing to install, and `around` never runs —
    /// `Repo` then fails closed on its own.
    fn capture(&self, req: &Request) -> Option<Captured> {
        // Coerce the request's own handle rather than boxing it again — the
        // ability is already an `Arc`, and this runs on every `/mcp` request.
        req.extensions()
            .get::<Arc<Ability>>()
            .cloned()
            .map(|ability| ability as Captured)
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        Box::pin(async move {
            // The guard installs the ambient ability on MCP exactly as it does
            // on GraphQL, so a tool body is scoped whether or not the app also
            // registered an `McpToolContext`.
            let Ok(ability) = captured.clone().downcast::<Ability>() else {
                // A downcast miss is a framework bug; run unscoped rather than
                // panic — `Repo` fails closed, same as the anonymous path.
                tracing::error!(
                    target: "nest_rs::authz",
                    reason = "guard_capture_downcast_miss",
                    "unexpected captured operation-guard state",
                );
                return inner.await;
            };
            with_ability(ability, inner).await
        })
    }
}
