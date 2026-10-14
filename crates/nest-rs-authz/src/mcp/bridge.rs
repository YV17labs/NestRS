//! [`McpAbilityBridge`] — the endpoint's operation guard.
//!
//! Authenticate MCP HTTP requests with the same guard chain controllers use;
//! the caller's ability it attaches is ambient for each operation the request
//! carries.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_guards::Guard;
use nest_rs_http::__private::denial_to_http_error;
use nest_rs_mcp::{BoxFuture, McpOperationGuard};
use poem::{Request, Result};

use crate::chain::run_ability_chain;

/// Runs `A` then `G` on each MCP HTTP request; what they attach — the caller's
/// ability — the endpoint installs around each operation. Inject it as
/// `dyn McpOperationGuard`; it joins the app's global guard pool, never
/// replaces it, and a pooled `A` or `G` runs once, here.
///
/// It gates the *request*, so it neither runs nor excuses an operation's own
/// `check` chain: a guard declared on a `#[tool]` — or in the app-wide pool —
/// still runs for the operation, even when this bridge already authenticated
/// the caller who sent it.
#[injectable]
pub struct McpAbilityBridge<A: Guard, G: Guard> {
    #[inject]
    auth: Arc<A>,
    #[inject]
    ability: Arc<G>,
}

impl<A: Guard, G: Guard> McpAbilityBridge<A, G> {
    const COVERS: [TypeId; 2] = [TypeId::of::<A>(), TypeId::of::<G>()];
}

impl<A: Guard, G: Guard> McpOperationGuard for McpAbilityBridge<A, G> {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            run_ability_chain(
                nest_rs_mcp::__private::endpoint_site(req),
                &*self.auth,
                &*self.ability,
                req,
            )
            .await
            .map_err(denial_to_http_error)
        })
    }

    fn covers(&self) -> &[TypeId] {
        &Self::COVERS
    }
}
