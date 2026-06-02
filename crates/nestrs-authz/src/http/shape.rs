//! Two transparent jobs `Authorize<A, S>` does as a [`RouteResponseShaper`], both
//! keyed on the caller's ability that the guard attached:
//!
//! 1. **Ambient ability** — [`run`](RouteResponseShaper::run) wraps the handler in
//!    [`with_ability`], so the data layer (`nestrs-database`'s `Repo`) reads the
//!    caller's ability via `current_ability()` and scopes every query — the
//!    developer writes no filter.
//! 2. **Response masking** — after the handler, the wire JSON is parsed into
//!    `S::Model` (filling columns the `#[expose]` output omits via the
//!    macro-emitted [`WireModelDefaults`] impl) and run through the same typed
//!    [`Ability::mask`] / [`Ability::mask_many`]. Keys absent from the wire body
//!    are stripped again so an unrestricted field grant cannot leak skipped
//!    columns (e.g. `password_hash`).
//!
//! Masking is a security control, so this fails *closed*: a successful JSON body
//! that cannot be reconciled with `S::Model` yields a `500` rather than shipping
//! the data unmasked. The whole body is buffered to mask it, so masked list
//! endpoints should be paginated.

use std::future::Future;
use std::sync::Arc;

use nestrs_http::RouteResponseShaper;
use nestrs_resource::WireModelDefaults;
use poem::http::StatusCode;
use poem::{Request, Response, Result};
use sea_orm::EntityTrait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use super::extractor::Authorize;
use crate::{with_ability, Ability, Action, ActionMarker};

impl<A, S> RouteResponseShaper for Authorize<A, S>
where
    A: ActionMarker,
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    type Captured = Option<Arc<Ability>>;

    fn capture(req: &Request) -> Self::Captured {
        req.extensions().get::<Arc<Ability>>().cloned()
    }

    async fn run<F>(captured: Self::Captured, inner: F) -> Result<Response>
    where
        F: Future<Output = Result<Response>> + Send,
    {
        // Install the ability as ambient state for the handler (and the services
        // it calls), so the data layer can scope queries; then mask what comes
        // back. A missing ability means the `Authorize` extractor already rejected
        // the request with a 500, so this branch only carries it through.
        match captured {
            Some(ability) => {
                let resp = with_ability(ability.clone(), inner).await?;
                Ok(mask_response::<S>(&ability, A::ACTION, resp).await)
            }
            None => inner.await,
        }
    }
}

/// Mask a successful JSON body: deserialize it into `S::Model`(s), run the typed
/// masking, and re-serialize. A non-success or non-JSON response, or a scalar
/// body, passes through; a JSON object/array that does not match `S::Model`
/// fails closed (see module docs).
async fn mask_response<S>(ability: &Ability, action: Action, mut resp: Response) -> Response
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    if !resp.status().is_success() {
        return resp;
    }
    let is_json = resp
        .content_type()
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return resp;
    }

    let bytes = match resp.take_body().into_bytes().await {
        Ok(bytes) => bytes,
        Err(_) => return resp,
    };

    // Parse the wire body first so we can round-trip handler DTOs (e.g. `User`
    // with string ids) into `S::Model` for policy, then drop any fields the wire
    // shape never carried (skipped columns like `password_hash`).
    let wire: Value = match serde_json::from_slice(bytes.as_ref()) {
        Ok(wire) => wire,
        Err(_) => {
            resp.set_body(bytes);
            return resp;
        }
    };

    let masked = match &wire {
        Value::Array(items) => {
            let models: Result<Vec<S::Model>, _> = items
                .iter()
                .map(|item| wire_to_model::<S>(item))
                .collect();
            models.map(|models| {
                let masked = ability.mask_many::<S>(action, models.iter());
                if masked.len() == items.len() {
                    Value::Array(
                        masked
                            .into_iter()
                            .zip(items.iter())
                            .map(|(mut row, wire_row)| {
                                retain_wire_keys(&mut row, wire_row);
                                row
                            })
                            .collect(),
                    )
                } else {
                    Value::Array(masked)
                }
            })
        }
        Value::Object(_) => wire_to_model::<S>(&wire).map(|model| {
                let mut masked = ability.mask::<S>(action, &model);
                retain_wire_keys(&mut masked, &wire);
                masked
            }),
        // Scalar / null — nothing to strip.
        _ => {
            resp.set_body(bytes);
            return resp;
        }
    };

    match masked.and_then(|value| serde_json::to_vec(&value)) {
        Ok(out) => {
            resp.set_body(out);
            resp
        }
        Err(_) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body("response masking failed: body did not match the authorized subject type"),
    }
}

/// Deserialize a handler JSON object into `S::Model`, filling the columns the
/// wire DTO omits so policy can run without a second subject type. The defaults
/// are placeholders, emitted by `#[expose]` from each `#[expose(skip)]`
/// scalar column's type — the masker strips those keys again via
/// [`retain_wire_keys`] before the response ships, so a default value never
/// reaches the wire.
fn wire_to_model<S>(wire: &Value) -> Result<S::Model, serde_json::Error>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned,
{
    if let Ok(model) = serde_json::from_value(wire.clone()) {
        return Ok(model);
    }
    let Value::Object(mut map) = wire.clone() else {
        return serde_json::from_value(wire.clone());
    };
    S::fill_wire_defaults(&mut map);
    serde_json::from_value(Value::Object(map))
}

/// Keep only keys the handler actually exposed on the wire (DTO fields), so an
/// unrestricted field grant cannot leak `#[expose(skip)]` columns.
fn retain_wire_keys(masked: &mut Value, wire: &Value) {
    let (Some(masked_obj), Some(wire_obj)) = (masked.as_object_mut(), wire.as_object()) else {
        return;
    };
    masked_obj.retain(|key, _| wire_obj.contains_key(key));
}
