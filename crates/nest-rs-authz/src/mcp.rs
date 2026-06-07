//! MCP surface for [`nest_rs_authz`](crate). Enabled by the `mcp` Cargo feature.
//!
//! Authenticate MCP HTTP requests with the same guard chain controllers use,
//! then install the caller's ambient [`Ability`] for the request duration.

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_guards::{Guard, integration::denial_to_http_response};
use nest_rs_mcp::{BoxFuture, McpOperationGuard};
use poem::http::StatusCode;
use poem::{Error, Request, Response, Result};

use crate::{Ability, with_ability};

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
            if self.auth.check_http(req).await.is_err() {
                return Err(Error::from_response(
                    Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body("Unauthorized"),
                ));
            }
            self.ability
                .check_http(req)
                .await
                .map_err(|denial| Error::from_response(denial_to_http_response(denial)))
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
