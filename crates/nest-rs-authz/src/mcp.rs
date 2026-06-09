//! MCP surface for [`nest_rs_authz`](crate). Enabled by the `mcp` Cargo feature.
//!
//! Authenticate MCP HTTP requests with the same guard chain controllers use,
//! then install the caller's ambient [`Ability`] for the request duration.

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_guards::{Guard, denial_to_http_response};
use nest_rs_mcp::{BoxFuture, McpOperationGuard};
use poem::http::StatusCode;
use poem::{Error, Request, Response, Result};
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Ability, ActionMarker, current_ability, with_ability};

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

/// Field-level response masking for MCP tools — the transport analog of
/// [`crate::http::mask_entity_response`] and `graphql::masked_output_for`.
///
/// MCP tool outputs are arbitrary JSON-RPC content, so masking can't be applied
/// transparently the way the HTTP route shaper does. A tool that returns an
/// entity row should route it through this helper so the same
/// `#[expose]`-policy field grants apply: it reads the ambient
/// [`Ability`] installed by [`McpAbilityBridge`], masks `model` for `A`, and
/// deserializes into the wire DTO `O`. With no ambient ability the call fails
/// closed (`Err`) rather than returning an unmasked row.
pub fn masked_output<A, E, O>(model: &E::Model) -> Result<O, serde_json::Error>
where
    A: ActionMarker,
    E: EntityTrait,
    E::Model: Serialize,
    O: DeserializeOwned,
{
    let masked = match current_ability() {
        Some(ability) => ability.mask::<E>(A::ACTION, model),
        // Fail closed: an MCP path with no ambient ability must not leak a
        // fully-populated row. Round-trip through an empty object so only
        // public (unrestricted) fields survive deserialization into `O`.
        None => serde_json::Value::Object(Default::default()),
    };
    serde_json::from_value(masked)
}
