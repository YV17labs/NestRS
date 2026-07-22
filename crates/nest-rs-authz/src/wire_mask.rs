//! Transport-shared wire-value masking: round-trip a handler's wire JSON
//! through the entity model so the typed [`Ability::mask`] policy can run,
//! then strain the result against the entity's statically-known exposed
//! columns ([`WireModelDefaults::wire_keys`]).
//!
//! The HTTP response shaper ([`crate::http::mask_entity_response`]) and the
//! GraphQL resolver wrapper ([`crate::graphql::masked_value_for`]) both
//! delegate here — one masking semantics for every transport, so the two
//! can't drift apart.

use nest_rs_resource::WireModelDefaults;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::{Deserialize, DeserializeOwned};
use serde_json::Value;

use crate::{Ability, Action};

// `warn_mask_failure` lives in `crate::ability` (always compiled) so the
// ambient `Ability::mask` can reach it in a feature-less build; re-exported
// here since the transport masking paths import it alongside `mask_wire_json`.
pub(crate) use crate::ability::warn_mask_failure;

/// Why [`masked_reply`] could not produce a masked value. Callers must treat
/// either case as fail-closed: send an error frame, never the unmasked body.
#[derive(Debug, thiserror::Error)]
pub enum MaskReplyError {
    /// No ambient [`Ability`] is installed — the auth bridge for this
    /// transport is missing, so masking cannot run.
    #[error("no ambient ability — is the transport's authz bridge installed?")]
    NoAmbientAbility,
    /// The wire value could not be reconciled with the entity model.
    #[error("wire value could not be reconciled with the entity model")]
    Irreconcilable(#[source] serde_json::Error),
}

/// Mask a handler's wire JSON with the **ambient** ability — the manual
/// analog of the HTTP response shaper and the GraphQL resolver wrapper, for
/// surfaces with no automatic shaper (a WS gateway reply, a hand-built
/// payload). One call replaces the hand-rolled serialize + permitted-fields +
/// retain dance, with the same fail-closed semantics: rows the ability
/// refuses are dropped, field grants strip columns, unexposed columns are
/// strained out, and an irreconcilable body is an error, never a passthrough.
///
/// # WS masking is opt-in — unlike HTTP and GraphQL (SEC-F1)
///
/// HTTP arms an automatic response shaper (and a `MaskProbe` compile/boot net);
/// GraphQL requires a `#[authorize]` that emits the mask. A WebSocket
/// `#[subscribe_message]` handler has **no** such net: a reply returning entity
/// data must call this helper itself and hand back the returned `Value`.
/// **Forgetting it ships raw, unmasked rows** — treat every entity-bearing WS
/// reply as requiring an explicit `masked_reply` call, and propagate the `Err`
/// to an error frame (never fall back to the unmasked body). See the
/// `demo/crates/features/src/users/ws/gateway.rs` exemplar.
pub fn masked_reply<S>(action: Action, wire: Value) -> Result<Value, MaskReplyError>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    let Some(ability) = crate::current_ability() else {
        return Err(MaskReplyError::NoAmbientAbility);
    };
    match mask_wire_json::<S>(&ability, action, &wire) {
        Ok(MaskedWire::Masked(masked)) => Ok(masked),
        Ok(MaskedWire::Passthrough) => Ok(wire),
        Err(err) => {
            warn_mask_failure(
                std::any::type_name::<S>(),
                action,
                "wire value could not be reconciled with the entity model",
                &err,
            );
            Err(MaskReplyError::Irreconcilable(err))
        }
    }
}

/// Outcome of masking one wire JSON value.
pub(crate) enum MaskedWire {
    /// An object or array body, masked and strained — ship this instead.
    Masked(Value),
    /// A scalar or `null` body — nothing entity-shaped to strip.
    Passthrough,
}

/// Mask a wire JSON value (an object or an array of objects) by
/// reconstructing each into `S::Model`, running [`Ability::mask`] /
/// [`Ability::mask_many`], and retaining only the exposed wire keys. Scalars
/// and `null` pass through. `Err` means the value could not be reconciled
/// with `S::Model` — callers must fail **closed**.
pub(crate) fn mask_wire_json<S>(
    ability: &Ability,
    action: Action,
    wire: &Value,
) -> Result<MaskedWire, serde_json::Error>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    let masked = match wire {
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
        Value::Object(_) => wire_to_model::<S>(wire).map(|model| {
            let mut masked = ability.mask::<S>(action, &model);
            match S::wire_keys() {
                Some(keys) => retain_static_keys(&mut masked, keys),
                None => retain_body_keys(&mut masked, wire),
            }
            masked
        }),
        // Scalar / null — nothing to strip.
        _ => return Ok(MaskedWire::Passthrough),
    };
    masked.map(MaskedWire::Masked)
}

/// Deserialize a handler JSON object into `S::Model`, filling columns the wire
/// DTO omits so policy can run. The placeholder defaults are stripped again by
/// [`retain_static_keys`] before the response ships — they never reach the wire.
///
/// Defaults are filled **before** the single deserialize rather than after a
/// speculative one: `fill_wire_defaults` only inserts keys the body is missing,
/// so the outcome is identical, while the straight-attempt-first shape used to
/// burn a whole clone and a doomed parse per row for every entity that hides a
/// non-`Option` column (`password_hash` — the common case).
fn wire_to_model<S>(wire: &Value) -> Result<S::Model, serde_json::Error>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned,
{
    let Value::Object(map) = wire else {
        // Not an object — nothing to fill; borrow-deserialize so the error path
        // costs no clone either.
        return S::Model::deserialize(wire);
    };
    let mut map = map.clone();
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
