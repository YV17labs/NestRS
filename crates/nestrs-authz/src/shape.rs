//! Transparent response masking: `Authorize<A, S>` implements
//! [`RouteResponseShaper`], so `#[routes]` rewrites a handler's response to drop
//! the rows and fields the caller's ability does not permit — no `mask` call in
//! the handler. The body is parsed back into `S::Model` and run through the same
//! typed [`Ability::mask`]/[`Ability::mask_many`] the engine already uses, so
//! there is no second masking implementation to keep in step.
//!
//! Masking is a security control, so this fails *closed*: a successful JSON body
//! that does not deserialize into `S::Model` (a handler/subject mismatch) yields
//! a `500` rather than shipping the data unmasked. The whole body is buffered to
//! mask it, so masked list endpoints should be paginated.

use std::sync::Arc;

use nestrs_http::RouteResponseShaper;
use poem::http::StatusCode;
use poem::{Request, Response};
use sea_orm::EntityTrait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::ability::Ability;
use crate::action::{Action, ActionMarker};
use crate::extractor::Authorize;

impl<A, S> RouteResponseShaper for Authorize<A, S>
where
    A: ActionMarker,
    S: EntityTrait,
    S::Model: DeserializeOwned + Serialize,
{
    type Captured = Option<Arc<Ability>>;

    fn capture(req: &Request) -> Self::Captured {
        req.extensions().get::<Arc<Ability>>().cloned()
    }

    async fn shape(captured: Self::Captured, resp: Response) -> Response {
        match captured {
            Some(ability) => mask_response::<S>(&ability, A::ACTION, resp).await,
            // Capture runs before the handler, whose `Authorize` extractor has
            // already rejected a missing ability with a 500 — so a successful
            // response here always carried one. Pass the (error) response through.
            None => resp,
        }
    }
}

/// Mask a successful JSON body: deserialize it into `S::Model`(s), run the typed
/// masking, and re-serialize. A non-success or non-JSON response, or a scalar
/// body, passes through; a JSON object/array that does not match `S::Model`
/// fails closed (see module docs).
async fn mask_response<S>(ability: &Ability, action: Action, mut resp: Response) -> Response
where
    S: EntityTrait,
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

    // Deserialize straight into the typed model(s) — array vs object by the first
    // non-whitespace byte — skipping a `Value` round-trip. `mask_many` also drops
    // rows the actor may not see; a single object is field-masked only (the
    // handler owns the instance-level allow/deny for a by-id route).
    let masked = match bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) {
        Some(b'[') => serde_json::from_slice::<Vec<S::Model>>(bytes.as_ref())
            .map(|models| Value::Array(ability.mask_many::<S>(action, models.iter()))),
        Some(b'{') => serde_json::from_slice::<S::Model>(bytes.as_ref())
            .map(|model| ability.mask::<S>(action, &model)),
        // Not a maskable object/array (scalar, null, empty) — nothing to strip.
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
