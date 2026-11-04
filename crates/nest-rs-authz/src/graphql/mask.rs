//! Field-level response masking for GraphQL resolvers — the transport analog of
//! [`crate::http::mask_entity_response`].
//!
//! The framework-carried path is [`masked_value_for`], emitted automatically by
//! `#[operations]` after every `#[authorize(Action, Entity)]`-declared operation.

use nest_rs_core::Edge;
use nest_rs_graphql::async_graphql::{Context, Error};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::ability;
use super::context::forbidden_fields;
use crate::ability::{Provenance, mask_reason};
use crate::gate::{Refusal, reason};
use crate::subject::{NoRows, Rows, SubjectMask};
use crate::wire_mask::{
    MaskedWire, judged, mask_row, mask_wire_detail, mask_wire_json, warn_mask_failure,
};
use crate::wire_model::WireModel;
use crate::{Ability, Action, ActionMarker};

/// Mask **one item of a subscription stream** through `ability`, and report
/// whether it survives — `Ok(None)` meaning this subscriber may not read this
/// item at all.
///
/// The streaming counterpart of the response mask, emitted automatically by
/// `#[operations]` after every `#[authorize(Action, Entity)]`-declared
/// `#[subscription]`, with the ability its class gate checked at the opening:
/// an item arrives outside that unit, where a guard bound on the field no
/// longer reaches. A resolver never calls it.
///
/// Each item is a row — the gate ran once, at subscribe — so one the ability
/// refuses is dropped, never nulled. No `Repo` read filtered it, so a rule
/// reading a relation or a column the item does not carry — an unexposed one,
/// or an exposed one its type leaves out — grants nothing and, as a denial,
/// denies. Fails **closed**: a value that cannot be reconciled with `E::Model`
/// is an `Err`, and the caller drops the item.
pub fn masked_item_for<A, E, O>(
    ctx: &Context<'_>,
    ability: &Ability,
    item: O,
) -> Result<Option<O>, Error>
where
    A: ActionMarker,
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let action = A::ACTION;
    let wire = serde_json::to_value(&item).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::NOT_SERIALIZABLE,
            "subscription item did not serialize",
            &err,
        )
    })?;
    // A scalar item has no row to evaluate; it passes as on the query path.
    if !wire.is_object() {
        return Ok(Some(item));
    }
    let verdict = judged::<E>(ability, action, &wire, Provenance::Pushed).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "subscription item could not be reconciled with the subject model",
            &err,
        )
    })?;
    if !verdict.allowed {
        return Ok(None);
    }
    let masked = mask_row::<E>(&verdict, wire);
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
/// round-trip (`crate::wire_mask`): serialize the value, read each object back
/// into `E::Model` through what `#[expose]` declares, judge it as
/// [`Ability::mask_many`] does, retain the exposed wire keys its verdict
/// grants, deserialize back. Handles the wire DTO itself, `Option<…>`,
/// `Vec<…>`; scalars and `None` pass through untouched.
///
/// A stripped **non-null** field cannot mask to `null`: when the operation
/// selected it, the operation is refused (`FORBIDDEN`, with the names in the
/// `fields` extension); otherwise the surviving rows are served. Rows the
/// ability refuses are dropped either way.
///
/// Fails **closed**: an irreconcilable value is a GraphQL error, never
/// unmasked data. The value is one the resolver read through `Repo` under this
/// ability, so a rule reading a relation or an unexposed column is the read's
/// to have decided.
///
/// [`Ability::mask_many`]: crate::Ability::mask_many
pub(crate) fn masked_value_for<A, E, O>(ctx: &Context<'_>, value: O) -> Result<O, Error>
where
    A: ActionMarker,
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let ability = ability()?;
    let action = A::ACTION;
    let wire = serde_json::to_value(&value).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::NOT_SERIALIZABLE,
            "resolver value did not serialize",
            &err,
        )
    })?;
    match mask_wire_json::<E>(&ability, action, wire, Provenance::Loaded) {
        Ok(MaskedWire::Passthrough(_)) => Ok(value),
        Ok(MaskedWire::Masked(masked)) => match serde_json::from_value(masked) {
            Ok(masked) => Ok(masked),
            // The mask took a key the wire type requires — not a failure in
            // itself, only if the operation asked for that key.
            Err(_) => unrepresentable::<E, O>(ctx, &ability, action, value),
        },
        Err(err) => Err(mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "value could not be reconciled with the subject model",
            &err,
        )),
    }
}

/// How an operation's posture masks its value on a subject's
/// [`SubjectMask::Mask`]: an entity's rows mask it, a subject no table holds
/// lets it through untouched.
#[diagnostic::on_unimplemented(
    message = "`#[authorize]` cannot mask a reply of type `{O}` on this subject",
    label = "the reply must serialize and deserialize, and this subject be an `#[expose]`d entity or a `#[subject]`",
    note = "a posture masks its reply against its entity's `#[expose]`d wire model, read and written as the entity's rows: expose the entity and answer a value that serializes and deserializes, or declare a capability no table holds with `#[subject]`, whose posture masks nothing"
)]
pub trait GraphqlMask<A, O> {
    /// The response mask's answer on an entity; the value itself otherwise.
    fn masked_value(ctx: &Context<'_>, value: O) -> Result<O, Error>;

    /// [`masked_item_for`]'s answer on an entity; the item itself otherwise.
    fn masked_item(ctx: &Context<'_>, ability: &Ability, item: O) -> Result<Option<O>, Error>;
}

#[diagnostic::do_not_recommend]
impl<A, E, O> GraphqlMask<A, O> for Rows<E>
where
    A: ActionMarker,
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    fn masked_value(ctx: &Context<'_>, value: O) -> Result<O, Error> {
        masked_value_for::<A, E, O>(ctx, value)
    }

    fn masked_item(ctx: &Context<'_>, ability: &Ability, item: O) -> Result<Option<O>, Error> {
        masked_item_for::<A, E, O>(ctx, ability, item)
    }
}

impl<A, O> GraphqlMask<A, O> for NoRows {
    fn masked_value(_ctx: &Context<'_>, value: O) -> Result<O, Error> {
        Ok(value)
    }

    fn masked_item(_ctx: &Context<'_>, _ability: &Ability, item: O) -> Result<Option<O>, Error> {
        Ok(Some(item))
    }
}

/// The value an `#[authorize(A, S)]` operation answers, masked as `S` masks it.
pub fn graphql_masked_value<A, S, O>(ctx: &Context<'_>, value: O) -> Result<O, Error>
where
    S: SubjectMask,
    S::Mask: GraphqlMask<A, O>,
{
    <S::Mask as GraphqlMask<A, O>>::masked_value(ctx, value)
}

/// One subscription item an `#[authorize(A, S)]` operation streams, masked as
/// `S` masks it through the ability its opening's gate checked.
pub fn graphql_masked_item<A, S, O>(
    ctx: &Context<'_>,
    ability: &Ability,
    item: O,
) -> Result<Option<O>, Error>
where
    S: SubjectMask,
    S::Mask: GraphqlMask<A, O>,
{
    <S::Mask as GraphqlMask<A, O>>::masked_item(ctx, ability, item)
}

/// The masked value did not fit the operation's own type: decide by selection
/// set (see [`masked_value_for`]).
fn unrepresentable<E, O>(
    ctx: &Context<'_>,
    ability: &Ability,
    action: Action,
    value: O,
) -> Result<O, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    // Written again rather than kept: the mask consumed the first, and only a
    // field grant the type cannot express comes this way.
    let wire = serde_json::to_value(&value).map_err(|err| {
        mask_failure::<E>(
            action,
            mask_reason::NOT_SERIALIZABLE,
            "resolver value did not serialize",
            &err,
        )
    })?;
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
/// A key outside the selection set is never serialized, so it cannot leak.
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
        ..Refusal::on(Edge::Graphql.as_str())
    });
    let refused: Vec<String> = refused.into_iter().map(str::to_owned).collect();
    Some(forbidden_fields(&refused))
}

/// One shape for every fail-closed masking exit: the queryable `warn` plus the
/// opaque client error, which never names the column or the reason.
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
        Some(Edge::Graphql.as_str()),
        None,
        Some(err),
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
