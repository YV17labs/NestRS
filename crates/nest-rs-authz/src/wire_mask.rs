//! Transport-shared wire-value masking: round-trip a handler's wire JSON
//! through the entity model so the typed [`Ability::mask`] policy can run,
//! then strain the result against the entity's statically-known exposed
//! columns ([`WireModelDefaults::wire_keys`]).

use crate::ability::mask_reason;
#[cfg(any(feature = "graphql", feature = "mcp"))]
use std::collections::BTreeSet;

use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::{Deserialize, DeserializeOwned};
use serde_json::Value;

#[cfg(any(feature = "graphql", feature = "mcp"))]
use crate::FieldSet;
use crate::error::MaskReplyError;
use crate::{Ability, Action, WireModelDefaults};

pub(crate) use crate::ability::warn_mask_failure;

/// Mask a handler's wire JSON with the **ambient** ability — the manual
/// analog of the HTTP response shaper and the GraphQL resolver wrapper, for
/// surfaces with no automatic shaper (a WS gateway reply, a hand-built
/// payload). One call replaces the hand-rolled serialize + permitted-fields +
/// retain dance, with the same fail-closed semantics: rows the ability
/// refuses are dropped, field grants strip columns, unexposed columns are
/// strained out, and an irreconcilable body is an error, never a passthrough.
///
/// Not for a decorated handler: `#[routes]`, `#[operations]`, `#[messages]` and
/// `#[tools]` arm their own mask from `#[authorize]`. This is for a surface no
/// decorator reaches (`WsServer::emit`, a hand-written `ServerHandler`):
/// propagate the `Err`, never fall back to the unmasked body.
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
                mask_reason::IRRECONCILABLE,
                "wire value could not be reconciled with the entity model",
                None,
                None,
                Some(&err),
            );
            Err(MaskReplyError::Irreconcilable(
                nest_rs_core::DecodeError::new(&err),
            ))
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
                    // Strain against the static exposed-column set: `mask_many`
                    // may drop rows, so `masked` no longer aligns with `items`.
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
        _ => return Ok(MaskedWire::Passthrough),
    };
    masked.map(MaskedWire::Masked)
}

/// What [`mask_wire_detail`] found: the rows that survived, still carrying
/// their original keys, and which keys the mask took off at least one of them.
#[cfg(any(feature = "graphql", feature = "mcp"))]
#[cfg_attr(
    not(feature = "graphql"),
    expect(
        dead_code,
        reason = "MCP asks only which keys went missing; `kept` and `dropped_rows` answer \
                  GraphQL's question of whether the surviving value can be handed back as is"
    )
)]
pub(crate) struct MaskedDetail {
    /// Surviving rows with their original keys — same row set as
    /// [`mask_wire_json`] produces, without the field-level stripping.
    pub(crate) kept: Value,
    /// Every key stripped from at least one surviving row, wire-spelled:
    /// refused by a field grant, or absent from the entity's exposed columns.
    pub(crate) removed: BTreeSet<String>,
    /// Whether any row was dropped — when none was, `kept` is the input
    /// verbatim and the caller can hand back the value it already holds
    /// instead of deserializing this one.
    pub(crate) dropped_rows: bool,
}

/// The row/field verdicts behind [`mask_wire_json`], reported instead of
/// applied.
#[cfg(any(feature = "graphql", feature = "mcp"))]
pub(crate) fn mask_wire_detail<S>(
    ability: &Ability,
    action: Action,
    wire: Value,
) -> Result<MaskedDetail, serde_json::Error>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    let mut removed = BTreeSet::new();
    let exposed = S::wire_keys();
    let mut dropped_rows = false;
    let kept = match wire {
        Value::Array(items) => {
            let mut kept = Vec::with_capacity(items.len());
            for item in items {
                let model = wire_to_model::<S>(&item)?;
                let verdict = ability.evaluate::<S>(action, &model);
                // Same verdict `mask_many` applies — a refused row is dropped,
                // never handed back for the caller to render.
                if !verdict.allowed {
                    dropped_rows = true;
                    continue;
                }
                collect_removed(&verdict.fields, exposed, &item, &mut removed);
                kept.push(item);
            }
            Value::Array(kept)
        }
        // A lone object is never dropped by `mask` (the class gate and `bind`
        // decide a singleton's visibility); only its fields go.
        other => {
            let model = wire_to_model::<S>(&other)?;
            let verdict = ability.evaluate::<S>(action, &model);
            collect_removed(&verdict.fields, exposed, &other, &mut removed);
            other
        }
    };
    Ok(MaskedDetail {
        kept,
        removed,
        dropped_rows,
    })
}

/// One masked row, plus the keys the mask took off it.
#[cfg(feature = "graphql")]
pub(crate) struct MaskedRow {
    /// The masked object, strained against the entity's exposed columns.
    pub(crate) masked: Value,
    /// Every key stripped from it, wire-spelled — what the caller needs to
    /// decide whether the *operation* asked for one of them.
    pub(crate) removed: BTreeSet<String>,
}

/// [`mask_wire_json`] for a single row whose model and verdict the caller
/// **already holds**.
///
/// Takes the verdict by value because [`Ability::mask_with`] consumes the field
/// set; `removed` is collected first, off the same one.
#[cfg(feature = "graphql")]
pub(crate) fn mask_row<S>(
    ability: &Ability,
    action: Action,
    model: &S::Model,
    verdict: crate::ability::Verdict,
    wire: &Value,
) -> MaskedRow
where
    S: EntityTrait + WireModelDefaults,
    S::Model: Serialize,
{
    let exposed = S::wire_keys();
    let mut removed = BTreeSet::new();
    collect_removed(&verdict.fields, exposed, wire, &mut removed);
    let mut masked = ability.mask_with::<S>(action, model, verdict.fields);
    match exposed {
        Some(keys) => retain_static_keys(&mut masked, keys),
        None => retain_body_keys(&mut masked, wire),
    }
    MaskedRow { masked, removed }
}

/// The keys [`mask_wire_json`] strips from this row: those outside the row's
/// field grant or the entity's statically exposed columns.
#[cfg(any(feature = "graphql", feature = "mcp"))]
fn collect_removed(
    granted: &FieldSet,
    exposed: Option<&'static [&'static str]>,
    row: &Value,
    out: &mut BTreeSet<String>,
) {
    let Some(obj) = row.as_object() else { return };
    for key in obj.keys() {
        let ungranted = match granted {
            FieldSet::All => false,
            FieldSet::Only(cols) => !cols.contains(key.as_str()),
        };
        let unexposed = exposed.is_some_and(|keys| !keys.contains(&key.as_str()));
        // `contains` first: every row of a list strips the same keys, so the
        // insert would otherwise allocate a `String` per row to keep one.
        if (ungranted || unexposed) && !out.contains(key.as_str()) {
            out.insert(key.clone());
        }
    }
}

/// Deserialize a handler JSON object into `S::Model`, filling columns the wire
/// DTO omits so policy can run. The placeholder defaults are stripped again by
/// [`retain_static_keys`] before the response ships — they never reach the wire.
///
pub(crate) fn wire_to_model<S>(wire: &Value) -> Result<S::Model, serde_json::Error>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned,
{
    let Value::Object(map) = wire else {
        // Not an object: nothing to fill, and no clone on the error path.
        return S::Model::deserialize(wire);
    };
    let mut map = map.clone();
    S::fill_wire_defaults(&mut map);
    serde_json::from_value(Value::Object(map))
}

/// Keep only the entity's statically-known exposed (`#[expose]`) columns, so
/// neither an unrestricted field grant nor a handler returning a raw `Model`
/// can leak an unexposed column. Keyed on the static set, it holds even when
/// `mask_many` drops rows.
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
