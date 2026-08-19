//! Field-level response masking for GraphQL resolvers — the transport analog of
//! [`crate::http::mask_entity_response`].
//!
//! The framework-carried path is [`masked_value_for`], emitted automatically by
//! `#[operations]` after every `#[authorize(Action, Entity)]`-declared operation —
//! a hand-written resolver never calls it.

use nest_rs_graphql::async_graphql::{Context, Error};
use nest_rs_resource::WireModelDefaults;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::ability;
use super::context::forbidden_fields;
use crate::ability::mask_reason;
use crate::gate::{Refusal, reason, transport};
use crate::wire_mask::{MaskedWire, mask_wire_detail, mask_wire_json, warn_mask_failure};
use crate::{Ability, Action, ActionMarker};

/// Mask **one item of a subscription stream** through the ambient ability, and
/// report whether it survives — `Ok(None)` meaning this subscriber may not read
/// this item at all.
///
/// The streaming counterpart of [`masked_value_for`], emitted automatically by
/// `#[operations]` after every `#[authorize(Action, Entity)]`-declared
/// `#[subscription]`. A resolver never calls it.
///
/// # Why it is not just `masked_value_for`
///
/// [`masked_value_for`] never drops a lone object: on a query the class gate and
/// `bind` have already decided instance visibility, so by the time the value is
/// masked the only open question is which *fields* the caller may read. A
/// subscription has no such decision per item — the gate ran once, at subscribe,
/// against the operation's class, and the items arrive afterwards from a stream
/// the subscriber did not address. So each one is a **row**, and the row-level
/// verdict applies exactly as `mask_many` applies it to a list: refused ⇒
/// dropped, never nulled and never masked to an empty shell. Two subscribers on
/// one stream therefore see two different item sequences, which is the whole
/// guarantee.
///
/// Fails **closed** on every uncertain case, like its sibling: a value that
/// cannot be reconciled with `E::Model` is an `Err`, and the caller
/// (`nest_rs_graphql::keep_masked_item`) drops the item rather than pushing it
/// unmasked.
pub fn masked_item_for<A, E, O>(ctx: &Context<'_>, item: O) -> Result<Option<O>, Error>
where
    A: ActionMarker,
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
    O: Serialize + DeserializeOwned,
{
    let ability = ability(ctx)?;
    let action = A::ACTION;
    let wire = serde_json::to_value(&item).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::NOT_SERIALIZABLE,
            "subscription item did not serialize",
            &err,
        )
    })?;
    // A scalar item (a counter, an id) is nothing entity-shaped: there is no row
    // to evaluate and no field to strip, so it passes exactly as it does on the
    // query path.
    if !wire.is_object() {
        return Ok(Some(item));
    }
    let model = crate::wire_mask::wire_to_model::<E>(&wire).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "subscription item could not be reconciled with the subject model",
            &err,
        )
    })?;
    // One rule scan answers both halves — whether this subscriber may see the
    // row, and which of its columns. Delegating to `masked_value_for` here would
    // re-serialize the item, rebuild the model and re-run this scan, on a path
    // that runs per item and per subscriber.
    let verdict = ability.evaluate::<E>(action, &model);
    if !verdict.allowed {
        return Ok(None);
    }
    let masked = crate::wire_mask::mask_row::<E>(&ability, action, &model, verdict, &wire);
    match serde_json::from_value(masked.masked) {
        Ok(masked) => Ok(Some(masked)),
        // The mask took a key the item's own type requires. Not a failure in
        // itself — only if the operation asked for that key.
        Err(_) => match refused_selection(ctx, action, std::any::type_name::<E>(), &masked.removed)
        {
            Some(denial) => Err(denial),
            None => Ok(Some(item)),
        },
    }
}

/// Mask a resolver's already-built wire value through the ambient ability —
/// the GraphQL analog of the HTTP response shaper, sharing its value-level
/// round-trip (`crate::wire_mask`): serialize the value, reconstruct each
/// object into `E::Model` (filling unexposed columns via
/// [`WireModelDefaults`]), run [`Ability::mask`] / [`Ability::mask_many`],
/// retain only the exposed wire keys, deserialize back. Handles the wire DTO
/// itself, `Option<…>`, `Vec<…>`; scalars and `None` pass through untouched.
///
/// # A stripped field the schema cannot null
///
/// The masked value comes back as the operation's own type, so a removed key
/// survives the round-trip only where the schema can express its absence — a
/// nullable field, which masks to `null` exactly as the HTTP body drops the
/// key. A **non-null** field (what `#[expose]` emits for a non-nullable column)
/// has no such representation, so GraphQL's own rule decides rather than the
/// value round-trip:
///
/// - the operation **selected** that field ⇒ refuse it (`FORBIDDEN`, with the
///   names in the `fields` extension): nulling it is not expressible, and
///   serving it would be the leak;
/// - it did **not** ⇒ serve the surviving rows. Only the selection set is ever
///   serialized, so a field outside it never reaches the client.
///
/// Rows the ability refuses are dropped either way. Without this the entity was
/// unreadable for every principal holding a partial field grant — a query
/// asking only for granted columns failed too, because what could not be
/// reconciled was the round-trip, not the response.
///
/// Fails **closed**: an irreconcilable value is a GraphQL error, never
/// unmasked data. Same caveat as HTTP masking: a hidden column an ability rule
/// predicates on is reconstructed from its [`WireModelDefaults`] default, so
/// such columns are best left exposed.
///
/// [`Ability::mask`]: crate::Ability::mask
/// [`Ability::mask_many`]: crate::Ability::mask_many
pub fn masked_value_for<A, E, O>(ctx: &Context<'_>, value: O) -> Result<O, Error>
where
    A: ActionMarker,
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
    O: Serialize + DeserializeOwned,
{
    let ability = ability(ctx)?;
    let action = A::ACTION;
    let wire = serde_json::to_value(&value).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::NOT_SERIALIZABLE,
            "resolver value did not serialize",
            &err,
        )
    })?;
    match mask_wire_json::<E>(&ability, action, &wire) {
        Ok(MaskedWire::Passthrough) => Ok(value),
        Ok(MaskedWire::Masked(masked)) => match serde_json::from_value(masked) {
            Ok(masked) => Ok(masked),
            // The mask took a key the wire type requires — not a failure in
            // itself, only if the operation asked for that key.
            Err(_) => unrepresentable::<E, O>(ctx, &ability, action, wire, value),
        },
        Err(err) => Err(mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "value could not be reconciled with the subject model",
            &err,
        )),
    }
}

/// The masked value did not fit the operation's own type: decide by selection
/// set (see [`masked_value_for`]). Takes the wire value and the original by
/// value — this is the steady-state path for a partial field grant, so it
/// neither clones the payload nor rebuilds one it already has.
fn unrepresentable<E, O>(
    ctx: &Context<'_>,
    ability: &Ability,
    action: Action,
    wire: serde_json::Value,
    value: O,
) -> Result<O, Error>
where
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
    O: DeserializeOwned,
{
    let detail = mask_wire_detail::<E>(ability, action, wire).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "value could not be reconciled with the subject model",
            &err,
        )
    })?;

    if let Some(denial) =
        refused_selection(ctx, action, std::any::type_name::<E>(), &detail.removed)
    {
        return Err(denial);
    }

    // Nothing refused and no row dropped ⇒ the surviving value *is* the one the
    // resolver returned, still owned here. Deserializing `kept` would rebuild it
    // key by key for nothing.
    if !detail.dropped_rows {
        return Ok(value);
    }
    serde_json::from_value(detail.kept).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "masked value did not match the authorized subject type",
            &err,
        )
    })
}

/// Whether the keys the mask stripped refuse the *operation*.
///
/// A stripped key only matters when the caller asked for it: GraphQL never
/// serializes a field outside the selection set, so one the operation did not
/// select cannot leak. The two masking paths — one value, one stream item —
/// answer this identically, and a second copy of "which removed key did the
/// client select?" is exactly the kind of divergence that turns into a leak on
/// one path and not the other.
///
/// The refusal is filed as a **denial**, through the emitter every gate refusal
/// uses, because that is what it is: this caller asked for a column this caller
/// may not read. MCP reaches the same decision without a selection set to weigh
/// it against, and files the same event — one message and one `reason` value,
/// so an incident query by `reason` returns both edges or neither.
fn refused_selection(
    ctx: &Context<'_>,
    action: Action,
    entity: &'static str,
    removed: &std::collections::BTreeSet<String>,
) -> Option<Error> {
    let selected: Vec<&str> = ctx.field().selection_set().map(|f| f.name()).collect();
    let refused: Vec<&str> = removed
        .iter()
        .filter(|key| selected.iter().any(|name| same_key(name, key)))
        .map(String::as_str)
        .collect();
    if refused.is_empty() {
        return None;
    }
    // A tracing field must be a scalar, so the log keeps the joined form; the
    // wire keeps the list.
    let joined = refused.join(",");
    crate::gate::warn_denied(Refusal {
        subject: Some(entity),
        action: Some(action),
        fields: Some(&joined),
        reason: Some(reason::FIELD_NOT_GRANTED),
        ..Refusal::on(transport::GRAPHQL)
    });
    let refused: Vec<String> = refused.into_iter().map(str::to_owned).collect();
    Some(forbidden_fields(&refused))
}

/// One shape for every fail-closed masking exit: the queryable `warn` (so a
/// branch that forgets it is the visible omission) plus the opaque client
/// error, which never names the column or the reason.
fn mask_failure<E>(
    action: Action,
    reason: &'static str,
    detail: &'static str,
    err: &serde_json::Error,
) -> Error {
    warn_mask_failure(
        std::any::type_name::<E>(),
        action,
        reason,
        detail,
        Some(transport::GRAPHQL),
        None,
        Some(&err),
    );
    Error::new("response masking failed: value did not match the authorized subject type")
}

/// Whether a GraphQL field name and a wire key name the same column, across the
/// two renamings that sit between them: async-graphql camelCases a schema
/// field, serde keeps the column's snake_case. Compared as folded character
/// streams — `orgId` and `org_id` are one key, with nothing allocated.
fn same_key(graphql: &str, wire: &str) -> bool {
    fn folded(name: &str) -> impl Iterator<Item = char> + '_ {
        name.chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
    }
    folded(graphql).eq(folded(wire))
}

#[cfg(test)]
mod tests {
    use super::same_key;

    #[test]
    fn the_camel_and_snake_spellings_of_a_column_are_one_key() {
        assert!(same_key("orgId", "org_id"));
        assert!(same_key("passwordHash", "password_hash"));
        assert!(same_key("id", "id"));
        assert!(!same_key("name", "email"));
    }
}
