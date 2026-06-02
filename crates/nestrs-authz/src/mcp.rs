//! MCP surface for [`nestrs_authz`](crate). Enabled by the `mcp` Cargo feature.
//!
//! Authenticate MCP HTTP requests with the same guard chain controllers use,
//! then install the caller's ambient [`Ability`] for the request duration.

use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_mcp::{BoxFuture, McpOperationGuard};
use nestrs_middleware::Guard;
use poem::http::StatusCode;
use poem::{Error, Request, Response, Result};

use crate::{with_ability, Ability};

/// Runs `A` then `G` on each MCP HTTP request and scopes the handler to the
/// resulting ability when present. Inject it as `dyn McpOperationGuard`.
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
            if self.auth.check(req).await.is_err() {
                return Err(Error::from_response(
                    Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body("Unauthorized"),
                ));
            }
            self.ability
                .check(req)
                .await
                .map_err(Error::from_response)
        })
    }
}

/// Re-install the caller's ability around the MCP handler when the guards attached
/// one — used by apps that wrap the endpoint beyond `before`.
pub async fn with_request_ability<F>(req: &Request, inner: F) -> Response
where
    F: std::future::Future<Output = Response>,
{
    match req.extensions().get::<Arc<Ability>>().cloned() {
        Some(ability) => with_ability(ability, inner).await,
        None => inner.await,
    }
}
