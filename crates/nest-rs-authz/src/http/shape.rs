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

    let masked = match &wire {
        Value::Array(items) => {
            let models: Result<Vec<S::Model>, _> =
                items.iter().map(|item| wire_to_model::<S>(item)).collect();
            models.map(|models| {
                let masked = ability.mask_many::<S>(action, models.iter());
                match S::wire_keys() {
                    // Strain every surviving row against the entity's static
                    // exposed-column set. `mask_many` may drop rows, so the
                    // masked vec no longer aligns with `items` by index — but
                    // the static key set needs no per-row body to strain
                    // against, which is exactly what closes the dropped-row leak.
                    Some(keys) => Value::Array(
                        masked
                            .into_iter()
                            .map(|mut row| {
                                retain_static_keys(&mut row, keys);
                                row
                            })
                            .collect(),
                    ),
                    // Opt-out entity (no `#[expose]`): we can only strain against
                    // the per-row body, which is sound only when nothing was
                    // dropped (index alignment preserved).
                    None => {
                        if masked.len() == items.len() {
                            Value::Array(
                                masked
                                    .into_iter()
                                    .zip(items.iter())
                                    .map(|(mut row, wire_row)| {
                                        retain_body_keys(&mut row, wire_row);
                                        row
                                    })
                                    .collect(),
                            )
                        } else {
                            Value::Array(masked)
                        }
                    }
                }
            })
        }
        Value::Object(_) => wire_to_model::<S>(&wire).map(|model| {
            let mut masked = ability.mask::<S>(action, &model);
            match S::wire_keys() {
                Some(keys) => retain_static_keys(&mut masked, keys),
                None => retain_body_keys(&mut masked, &wire),
            }
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

/// Deserialize a handler JSON object into `S::Model`, filling columns the wire
/// DTO omits so policy can run. The placeholder defaults are stripped again by
/// [`retain_static_keys`] before the response ships — they never reach the wire.
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

/// Keep only the entity's statically-known exposed (`#[expose]`) columns, so
/// neither an unrestricted field grant nor a handler returning a raw `Model`
/// can leak an unexposed column. Keying on the static set (not the response
/// body) is what makes this hold even when `mask_many` drops rows, and it cuts
/// a raw-`Model` body down to its exposed columns rather than trusting it.
fn retain_static_keys(masked: &mut Value, keys: &'static [&'static str]) {
    if let Some(masked_obj) = masked.as_object_mut() {
        masked_obj.retain(|key, _| keys.contains(&key.as_str()));
    }
}

/// Fallback strainer for entities that opt out of [`WireModelDefaults::wire_keys`]
/// (no `#[expose]`): keep only keys the response body already carried. Sound
/// only when the body is itself the wire shape and rows weren't dropped.
fn retain_body_keys(masked: &mut Value, wire: &Value) {
    let (Some(masked_obj), Some(wire_obj)) = (masked.as_object_mut(), wire.as_object()) else {
        return;
    };
    masked_obj.retain(|key, _| wire_obj.contains_key(key));
}
