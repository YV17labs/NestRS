//! Transport-shared wire-value masking: round-trip a handler's wire JSON
//! through the entity model so the typed [`Ability::mask`] policy can run,
//! then strain the result against the entity's statically-known exposed
//! columns ([`WireModelDefaults::wire_keys`]).

use crate::ability::{Provenance, mask_reason};
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

/// Mask a pushed wire value with the **ambient** ability — the manual analog of
/// the masks `#[authorize]` arms, for a surface no decorator reaches (a
/// hand-written `ServerHandler`, a value a WS handler builds for its own
/// caller). Propagate the `Err`, never fall back to the unmasked body.
///
/// Rows the ability refuses are dropped, and a lone object it refuses comes back
/// `null`; field grants strip columns; what ships is the exposed keys the value
/// carried, never one the rebuild filled; an irreconcilable body is an error,
/// never a passthrough.
///
/// The value is judged as a **push**: no `Repo` read decided a relation or a
/// column the value does not carry — unexposed, or exposed and left out — so a
/// rule reading one grants nothing and a denial reading one denies. Decide such
/// a rule before pushing, or expose the column and send it.
///
/// It masks for the principal whose ability is ambient — right for a reply to
/// that principal. A push to other connections (`WsServer::emit`,
/// `broadcast`) carries what this caller may see, not what each recipient may:
/// the framework does not mask per recipient.
pub fn masked_reply<S>(action: Action, wire: Value) -> Result<Value, MaskReplyError>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    let Some(ability) = crate::current_ability() else {
        return Err(MaskReplyError::NoAmbientAbility);
    };
    match mask_wire_json::<S>(&ability, action, &wire, Provenance::Pushed) {
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
/// reconstructing each into `S::Model`, judging it as `provenance` says, and
/// retaining only the exposed wire keys it carried. Scalars and `null` pass
/// through. `Err` means the value could not be reconciled with `S::Model` —
/// callers must fail **closed**.
pub(crate) fn mask_wire_json<S>(
    ability: &Ability,
    action: Action,
    wire: &Value,
    provenance: Provenance,
) -> Result<MaskedWire, serde_json::Error>
where
    S: EntityTrait + WireModelDefaults,
    S::Model: DeserializeOwned + Serialize,
{
    let masked = match wire {
        Value::Array(items) => {
            let mut masked = Vec::with_capacity(items.len());
            for item in items {
                let model = wire_to_model::<S>(item)?;
                let verdict =
                    ability.evaluate::<S>(action, &model, provenance, &invented_in::<S>(item));
                if verdict.allowed {
                    masked.push(strained::<S>(
                        ability.mask_with::<S>(action, &model, verdict.fields),
                        item,
                    ));
                }
            }
            Value::Array(masked)
        }
        Value::Object(_) => {
            let model = wire_to_model::<S>(wire)?;
            let verdict =
                ability.evaluate::<S>(action, &model, provenance, &invented_in::<S>(wire));
            // A reply's lone row is the one its read admitted; a pushed one had
            // no read, so it is judged like any row of a list.
            if provenance == Provenance::Pushed && !verdict.allowed {
                Value::Null
            } else {
                strained::<S>(ability.mask_with::<S>(action, &model, verdict.fields), wire)
            }
        }
        _ => return Ok(MaskedWire::Passthrough),
    };
    Ok(MaskedWire::Masked(masked))
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

/// The row/field verdicts behind [`mask_wire_json`] on a decorated reply,
/// reported instead of applied.
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
                let verdict = ability.evaluate::<S>(
                    action,
                    &model,
                    Provenance::Loaded,
                    &invented_in::<S>(&item),
                );
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
            let verdict = ability.evaluate::<S>(
                action,
                &model,
                Provenance::Loaded,
                &invented_in::<S>(&other),
            );
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
    let mut removed = BTreeSet::new();
    collect_removed(&verdict.fields, S::wire_keys(), wire, &mut removed);
    let masked = strained::<S>(ability.mask_with::<S>(action, model, verdict.fields), wire);
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
/// [`strained`] before the response ships — they never reach the wire.
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

/// The columns a row rebuilt from `wire` was given rather than sent: one the
/// fill may invent, and one the object left out, which serde defaulted.
pub(crate) fn invented_in<S: WireModelDefaults>(wire: &Value) -> impl Fn(S::Column) -> bool + '_ {
    let sent = wire.as_object();
    move |column| {
        S::is_placeholdered(column)
            || S::wire_key(column).is_some_and(|key| !sent.is_some_and(|row| row.contains_key(key)))
    }
}

/// A masked row cut back to the keys its wire row carried, among the entity's
/// exposed (`#[expose]`) columns when it states them: neither an unrestricted
/// field grant nor a handler returning a raw `Model` leaks an unexposed column,
/// and no key the row left out comes back with the value the rebuild gave it.
fn strained<S: WireModelDefaults>(mut masked: Value, wire: &Value) -> Value {
    let exposed = S::wire_keys();
    let sent = wire.as_object();
    if let Some(row) = masked.as_object_mut() {
        row.retain(|key, _| {
            sent.is_some_and(|sent| sent.contains_key(key))
                && exposed.is_none_or(|keys| keys.contains(&key.as_str()))
        });
    }
    masked
}
