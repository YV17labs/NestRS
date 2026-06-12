//! `Authorize<A, S>` as a [`RouteResponseShaper`] does two things keyed on the
//! ability the guard attached:
//!
//! 1. Wraps the handler in [`with_ability`] so the data layer scopes every
//!    query via `current_ability()` — no hand-written filter.
//! 2. Masks the JSON response: parse into `S::Model` (filling the unexposed
//!    columns the `#[expose]` output omits via [`WireModelDefaults`]), run typed
//!    [`Ability::mask`] / [`Ability::mask_many`], then retain only the entity's
//!    statically-known exposed columns ([`WireModelDefaults::wire_keys`]) so
//!    neither an unrestricted field grant nor a handler returning a raw `Model`
//!    can leak an unexposed column (e.g. `password_hash`, which carries no
//!    `#[expose]`). Keying on the static set rather than the response body keeps
//!    this sound even when `mask_many` drops rows.
//!
//! Fails **closed**: a successful JSON body that cannot be reconciled with
//! `S::Model` yields 500 rather than shipping data unmasked.

use std::future::Future;
use std::sync::Arc;

use nest_rs_http::RouteResponseShaper;
use nest_rs_resource::WireModelDefaults;
use poem::http::StatusCode;
use poem::{Request, Response, Result};
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::extractor::Authorize;
use crate::wire_mask::{MaskedWire, mask_wire_json};
use crate::{Ability, Action, ActionMarker, with_ability};

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
        // A missing ability means the extractor already rejected with 500.
        match captured {
            Some(ability) => {
                let resp = with_ability(ability.clone(), inner).await?;
                Ok(mask_entity_response::<S>(&ability, A::ACTION, resp).await)
            }
            None => inner.await,
        }
    }
}

/// Mask a successful JSON body: deserialize it into `S::Model`(s), run the typed
/// masking, and re-serialize. A non-success or non-JSON response, or a scalar
/// body, passes through; a JSON object/array that does not match `S::Model`
/// fails closed (see module docs).
pub async fn mask_entity_response<S>(
    ability: &Ability,
    action: Action,
    mut resp: Response,
) -> Response
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

    // Round-trip handler DTOs through `S::Model` for policy, then drop any
    // fields the wire shape never carried (e.g. `password_hash`).
    let wire: Value = match serde_json::from_slice(bytes.as_ref()) {
        Ok(wire) => wire,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("response masking failed: body was not valid JSON");
        }
    };

    // One masking semantics for every transport: the value-level round-trip
    // (wire → `S::Model` → mask → exposed-key strainer) lives in
    // `crate::wire_mask`, shared with the GraphQL resolver wrapper.
    match mask_wire_json::<S>(ability, action, &wire) {
        Ok(MaskedWire::Passthrough) => {
            // Scalar / null — nothing to strip.
            resp.set_body(bytes);
            resp
        }
        Ok(MaskedWire::Masked(value)) => match serde_json::to_vec(&value) {
            Ok(out) => {
                resp.set_body(out);
                resp
            }
            Err(_) => masking_failed(),
        },
        Err(_) => masking_failed(),
    }
}

/// The fail-closed response: a successful body that cannot be reconciled with
/// the authorized subject type ships a 500, never unmasked data.
fn masking_failed() -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body("response masking failed: body did not match the authorized subject type")
}
