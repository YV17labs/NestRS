//! Transport-shared wire-value masking: each row of a handler's wire JSON is
//! read back into the entity's model, through what `#[expose]` declares, so
//! the ability's rules run on it, then cut to the exposed keys its verdict
//! grants.

use crate::ability::{Provenance, Verdict, mask_reason};
#[cfg(any(feature = "graphql", feature = "mcp"))]
use std::collections::BTreeSet;

use serde::de::{Error as _, Unexpected};
use serde_json::{Map, Value};

use crate::error::MaskReplyError;
use crate::wire_model::{WireModel, kept_key};
use crate::{Ability, Action, FieldSet};

pub(crate) use crate::ability::{MaskedUnit, warn_mask_failure};

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
/// a rule before pushing, or expose the column and send it. [`Action::Manage`]
/// judges it at [`Action::Read`].
///
/// It masks for the principal whose ability is ambient — right for a reply to
/// that principal. A push to other connections (`WsServer::emit`,
/// `broadcast`) carries what this caller may see, not what each recipient may:
/// the framework does not mask per recipient.
pub fn masked_reply<S: WireModel>(action: Action, wire: Value) -> Result<Value, MaskReplyError> {
    let Some(ability) = crate::current_ability() else {
        return Err(MaskReplyError::NoAmbientAbility);
    };
    match mask_wire_json::<S>(&ability, action, wire, Provenance::Pushed) {
        Ok(MaskedWire::Masked(value) | MaskedWire::Passthrough(value)) => Ok(value),
        Err(err) => {
            warn_mask_failure(
                S::default().table_name(),
                action,
                mask_reason::IRRECONCILABLE,
                "wire value could not be reconciled with the entity model",
                MaskedUnit::default(),
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
    /// A scalar or `null` body, handed back — nothing entity-shaped to strip.
    Passthrough(Value),
}

/// Mask a wire JSON value (an object or an array of objects) by reading each
/// row back into `S::Model`, judging it as `provenance` says, and retaining
/// only the exposed wire keys it carried that the verdict grants, in place.
/// Scalars and `null` pass through. `Err` means the value could not be
/// reconciled with `S::Model` — callers must fail **closed**.
pub(crate) fn mask_wire_json<S: WireModel>(
    ability: &Ability,
    action: Action,
    wire: Value,
    provenance: Provenance,
) -> Result<MaskedWire, serde_json::Error> {
    let masked = match wire {
        Value::Array(items) => {
            let mut masked = Vec::with_capacity(items.len());
            for mut item in items {
                // No row, so nothing to judge or to leak.
                if item.is_null() {
                    masked.push(item);
                    continue;
                }
                let verdict = judged::<S>(ability, action, &item, provenance)?;
                if verdict.allowed {
                    retain_granted::<S>(&mut item, &verdict.fields);
                    masked.push(item);
                }
            }
            Value::Array(masked)
        }
        mut row @ Value::Object(_) => {
            let verdict = judged::<S>(ability, action, &row, provenance)?;
            // A reply's lone row is the one its read admitted; a pushed one had
            // no read, so it is judged like any row of a list.
            if provenance == Provenance::Pushed && !verdict.allowed {
                Value::Null
            } else {
                retain_granted::<S>(&mut row, &verdict.fields);
                row
            }
        }
        other => return Ok(MaskedWire::Passthrough(other)),
    };
    Ok(MaskedWire::Masked(masked))
}

/// The verdict on one wire row, read back into `S::Model` for the rules to run
/// on — its exposed columns from the object, every other one from the
/// placeholder `#[expose]` gave it, which never reaches the wire — the columns
/// it was given rather than sent named as invented.
pub(crate) fn judged<S: WireModel>(
    ability: &Ability,
    action: Action,
    row: &Value,
    provenance: Provenance,
) -> Result<Verdict, serde_json::Error> {
    let Value::Object(row) = row else {
        return Err(serde_json::Error::invalid_type(
            Unexpected::Other(json_kind(row)),
            &"a row object",
        ));
    };
    let model = S::from_wire(row)?;
    Ok(ability.evaluate::<S>(action, &model, provenance, &invented_in::<S>(row)))
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

/// The row/field verdicts behind [`mask_wire_json`], reported instead of applied.
#[cfg(any(feature = "graphql", feature = "mcp"))]
pub(crate) fn mask_wire_detail<S: WireModel>(
    ability: &Ability,
    action: Action,
    wire: Value,
    provenance: Provenance,
) -> Result<MaskedDetail, serde_json::Error> {
    let mut removed = BTreeSet::new();
    let mut dropped_rows = false;
    let kept = match wire {
        Value::Array(items) => {
            let mut kept = Vec::with_capacity(items.len());
            for item in items {
                if item.is_null() {
                    kept.push(item);
                    continue;
                }
                let verdict = judged::<S>(ability, action, &item, provenance)?;
                // Same verdict `mask_many` applies — a refused row is dropped,
                // never handed back for the caller to render.
                if !verdict.allowed {
                    dropped_rows = true;
                    continue;
                }
                collect_removed::<S>(&verdict.fields, &item, &mut removed);
                kept.push(item);
            }
            Value::Array(kept)
        }
        // A lone object is never dropped by `mask` (the class gate and `bind`
        // decide a singleton's visibility); only its fields go. A pushed one had
        // no read to admit it, so what refuses it takes every field it holds.
        other => {
            let verdict = judged::<S>(ability, action, &other, provenance)?;
            let fields = if provenance == Provenance::Pushed && !verdict.allowed {
                FieldSet::Only(Default::default())
            } else {
                verdict.fields
            };
            collect_removed::<S>(&fields, &other, &mut removed);
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

/// [`mask_wire_json`] for a single row whose verdict the caller **already
/// holds**.
#[cfg(feature = "graphql")]
pub(crate) fn mask_row<S: WireModel>(verdict: &Verdict, mut wire: Value) -> MaskedRow {
    let mut removed = BTreeSet::new();
    collect_removed::<S>(&verdict.fields, &wire, &mut removed);
    retain_granted::<S>(&mut wire, &verdict.fields);
    MaskedRow {
        masked: wire,
        removed,
    }
}

/// The keys [`mask_wire_json`] strips from this row: those outside the row's
/// field grant or the entity's exposed columns.
#[cfg(any(feature = "graphql", feature = "mcp"))]
fn collect_removed<S: WireModel>(granted: &FieldSet, row: &Value, out: &mut BTreeSet<String>) {
    let Some(obj) = row.as_object() else { return };
    for key in obj.keys() {
        // `contains` first: every row of a list strips the same keys, so the
        // insert would otherwise allocate a `String` per row to keep one.
        if !kept_key::<S>(granted, key) && !out.contains(key.as_str()) {
            out.insert(key.clone());
        }
    }
}

/// The kind of a JSON value, never its content: a refusal names what it met.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The columns a row rebuilt from `row` was given rather than sent: one the
/// rebuild invents, and one the object left out.
fn invented_in<S: WireModel>(row: &Map<String, Value>) -> impl Fn(S::Column) -> bool + '_ {
    move |column| {
        S::is_placeholdered(column) || S::wire_key(column).is_some_and(|key| !row.contains_key(key))
    }
}

/// Cut a wire row to the keys it carried that are the entity's exposed
/// (`#[expose]`) columns and that `fields` grants: neither an unrestricted
/// field grant nor a body carrying a hidden column ships one.
fn retain_granted<S: WireModel>(row: &mut Value, fields: &FieldSet) {
    if let Value::Object(row) = row {
        row.retain(|key, _| kept_key::<S>(fields, key));
    }
}
