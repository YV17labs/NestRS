//! Field-level response masking for GraphQL resolvers — the transport analog of
//! [`crate::http::mask_entity_response`].
//!
//! The framework-carried path is [`masked_value_for`], emitted automatically by
//! `#[operations]` after every `#[authorize(Action, Entity)]`-declared operation.
//! A nested field runs off that unit and masks what it answers by the same rule:
//! [`graphql_masked_row`] and [`graphql_masked_nodes`] on an `#[expose]`d relation,
//! [`graphql_masked_answer`] and [`graphql_masked_answer_nodes`] on a
//! `#[field_resolver]` answering a wire type.

use std::any::TypeId;
use std::collections::BTreeSet;

use nest_rs_core::__private::Site;
use nest_rs_core::Edge;
use nest_rs_graphql::__private::{first_in_operation, schema_field_spelling, served_field};
use nest_rs_graphql::async_graphql::{Context, Error, SelectionField};
use nest_rs_guards::Denial;
use sea_orm::{EntityTrait, Iterable, PrimaryKeyToColumn};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::context::{field_ability, forbidden_fields, unreached};
use crate::ability::{Provenance, mask_reason};
use crate::gate::{Refusal, reason};
use crate::subject::{NoRows, Rows, SubjectMask};
use crate::wire_mask::{
    MaskedUnit, MaskedWire, judged, mask_row, mask_wire_detail, mask_wire_json, warn_mask_failure,
};
use crate::wire_model::WireModel;
use crate::{Ability, Action, ActionMarker};

/// Mask **one item of a subscription stream** through `ability`, and report
/// whether it survives — `Ok(None)` meaning this subscriber may not read this
/// item at all.
///
/// The streaming counterpart of the response mask. `#[operations]` runs the same
/// mask after every `#[authorize(Action, Entity)]`-declared `#[subscription]`,
/// with the ability its class gate checked at the opening: an item arrives outside
/// that unit, where a guard bound on the field no longer reaches. A resolver calls
/// it only for an item that posture does not cover.
///
/// Each item is a row — the gate ran once, at subscribe — so one the ability
/// refuses is dropped, never nulled, as a refused row of a list item is. No `Repo`
/// read filtered it, so a rule reading a relation or a column the item does not
/// carry — an unexposed one, or an exposed one its type leaves out — grants nothing
/// and, as a denial, denies. Fails **closed**: a value that cannot be reconciled
/// with `E::Model` is an `Err`, and the caller drops the item.
///
/// A body calling this names no site, so what it refuses says the `kind` and
/// `field` its context holds, and no host, as a relation's refusal does.
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
    masked_item_on::<E, O>(MaskedField::of(ctx, None), A::ACTION, ability, item)
}

/// [`masked_item_for`] for the `field` a decorator's site names.
fn masked_item_on<E, O>(
    field: MaskedField<'_, '_>,
    action: Action,
    ability: &Ability,
    mut item: O,
) -> Result<Option<O>, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let wire = written::<E, O>(field, action, &item)?;
    if wire.is_array() {
        return masked_value_with::<E, O>(field, ability, action, item, Provenance::Pushed)
            .map(Some);
    }
    let kept = masked_in_place::<E, O>(
        field,
        ability,
        action,
        &mut item,
        wire,
        Provenance::Pushed,
        |removed| refused_field::<E>(field, action, removed),
    )?;
    Ok(kept.then_some(item))
}

/// One row, `wire` what it serializes to, through the mask: `false` when the
/// ability refuses it, the row cut to what it grants when its type can read that,
/// and otherwise the row as it stands, unless `refused` says the keys the mask took
/// off it refuse the field.
fn masked_in_place<E, O>(
    field: MaskedField<'_, '_>,
    ability: &Ability,
    action: Action,
    row: &mut O,
    wire: Value,
    provenance: Provenance,
    refused: impl FnOnce(&BTreeSet<String>) -> Option<Error>,
) -> Result<bool, Error>
where
    E: WireModel,
    O: DeserializeOwned,
{
    // A scalar has no row to evaluate; it passes as on the query path.
    if !wire.is_object() {
        return Ok(true);
    }
    let verdict = judged::<E>(ability, action, &wire, provenance).map_err(|err| {
        mask_failure::<E>(
            field,
            action,
            mask_reason::IRRECONCILABLE,
            "row could not be reconciled with the subject model",
            &err,
        )
    })?;
    if !verdict.allowed {
        return Ok(false);
    }
    let masked = mask_row::<E>(&verdict, wire);
    match serde_json::from_value(masked.masked) {
        Ok(cut) => {
            *row = cut;
            Ok(true)
        }
        // The mask took a key the row's own type requires. Not a failure in
        // itself — only if the field asked for that key.
        Err(_) => match refused(&masked.removed) {
            Some(denial) => Err(denial),
            None => Ok(true),
        },
    }
}

/// [`masked_in_place`] over a row held by value: `Ok(None)` when the ability refuses it.
fn masked_row<E, O>(
    field: MaskedField<'_, '_>,
    ability: &Ability,
    action: Action,
    mut row: O,
    provenance: Provenance,
    refused: impl FnOnce(&BTreeSet<String>) -> Option<Error>,
) -> Result<Option<O>, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let wire = written::<E, O>(field, action, &row)?;
    let kept =
        masked_in_place::<E, O>(field, ability, action, &mut row, wire, provenance, refused)?;
    Ok(kept.then_some(row))
}

/// What a relation's field answers of the row its batch read, by this field's own
/// selection: a batch is shared by every alias asking, so it filters rows and
/// leaves the columns to here. A row the ability refuses reads `None`.
///
/// The batch read the row through `Repo` under this ability, into the entity's own
/// wire type, so a caller whose grants narrow no column pays for no round trip.
pub fn graphql_masked_row<E, O>(ctx: &Context<'_>, row: Option<O>) -> Result<Option<O>, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let Some(row) = row else {
        return Ok(None);
    };
    let ability = field_ability(ctx)?;
    if ability.grants_every_column(Action::Read, TypeId::of::<E>()) {
        return Ok(Some(row));
    }
    let field = MaskedField::of(ctx, None);
    masked_row::<E, O>(
        field,
        &ability,
        Action::Read,
        row,
        Provenance::Loaded,
        |removed| refused_field::<E>(field, Action::Read, removed),
    )
}

/// [`graphql_masked_row`] over one page of a list relation, each row beside its
/// cursor: a row the ability refuses leaves with its cursor.
pub fn graphql_masked_nodes<E, O>(
    ctx: &Context<'_>,
    nodes: Vec<(String, O)>,
) -> Result<Vec<(String, O)>, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    if nodes.is_empty() {
        return Ok(nodes);
    }
    let ability = field_ability(ctx)?;
    if ability.grants_every_column(Action::Read, TypeId::of::<E>()) {
        return Ok(nodes);
    }
    let selected = page_selection::<E>(ctx);
    let field = MaskedField::of(ctx, None);
    let mut kept = Vec::with_capacity(nodes.len());
    for (cursor, node) in nodes {
        let node = masked_row::<E, O>(
            field,
            &ability,
            Action::Read,
            node,
            Provenance::Loaded,
            |removed| refused_selection::<E>(field, &selected, Action::Read, removed),
        )?;
        if let Some(node) = node {
            kept.push((cursor, node));
        }
    }
    Ok(kept)
}

/// What a `#[field_resolver]` answering a wire type answers, masked by a root's
/// rule, its action a read. Where no ability is attached nothing decided what the
/// caller reads, so it fails closed, as a root and a relation do.
pub fn graphql_masked_answer<E, O>(
    ctx: &Context<'_>,
    site: &'static Site,
    answer: O,
) -> Result<O, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let ability = crate::context::installed().ok_or_else(|| unreached(ctx))?;
    masked_value_with::<E, O>(
        MaskedField::of(ctx, Some(site)),
        &ability,
        Action::Read,
        answer,
        answer_provenance(&ability),
    )
}

/// [`graphql_masked_answer`] over the rows of a connection a body built, each masked
/// in place by the connection's selection: whether each stays, the rest of its edge
/// being the body's.
pub fn graphql_masked_answer_nodes<E, O>(
    ctx: &Context<'_>,
    site: &'static Site,
    nodes: Vec<&mut O>,
) -> Result<Vec<bool>, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let ability = crate::context::installed().ok_or_else(|| unreached(ctx))?;
    let provenance = answer_provenance(&ability);
    let selected = page_selection::<E>(ctx);
    let field = MaskedField::of(ctx, Some(site));
    nodes
        .into_iter()
        .map(|node| {
            let wire = written::<E, O>(field, Action::Read, &*node)?;
            masked_in_place::<E, O>(
                field,
                &ability,
                Action::Read,
                node,
                wire,
                provenance,
                |removed| refused_selection::<E>(field, &selected, Action::Read, removed),
            )
        })
        .collect()
}

/// How a body's answer is judged. A caller a token backs reads through `Repo` under
/// this ability, so its answer is a reply, keeping the trust a read earns whatever
/// read it. A visitor's reads are withheld below its root, so no read can have
/// applied its rules, and its answer is a push.
fn answer_provenance(ability: &Ability) -> Provenance {
    if ability.is_visitor() {
        Provenance::Pushed
    } else {
        Provenance::Loaded
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
/// selected it, or the field selects nothing, a scalar serializing whole, the
/// operation is refused (`FORBIDDEN`, with the names in the `fields` extension);
/// otherwise the surviving rows are served. Rows the ability refuses are dropped
/// either way.
///
/// Fails **closed**: an irreconcilable value is a GraphQL error, never
/// unmasked data. The value is one the resolver read through `Repo` under this
/// ability, so a rule reading a relation or an unexposed column is the read's
/// to have decided.
///
/// [`Ability::mask_many`]: crate::Ability::mask_many
pub(crate) fn masked_value_for<A, E, O>(
    ctx: &Context<'_>,
    site: &'static Site,
    value: O,
) -> Result<O, Error>
where
    A: ActionMarker,
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let ability = field_ability(ctx)?;
    let field = MaskedField::of(ctx, Some(site));
    masked_value_with::<E, O>(field, &ability, A::ACTION, value, Provenance::Loaded)
}

/// [`masked_value_for`] under the ability its caller read, judged as `provenance` says.
fn masked_value_with<E, O>(
    field: MaskedField<'_, '_>,
    ability: &Ability,
    action: Action,
    value: O,
    provenance: Provenance,
) -> Result<O, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    let wire = written::<E, O>(field, action, &value)?;
    match mask_wire_json::<E>(ability, action, wire, provenance) {
        Ok(MaskedWire::Passthrough(_)) => Ok(value),
        Ok(MaskedWire::Masked(masked)) => match serde_json::from_value(masked) {
            Ok(masked) => Ok(masked),
            // The mask took a key the wire type requires — not a failure in
            // itself, only if the operation asked for that key.
            Err(_) => unrepresentable::<E, O>(field, ability, action, value, provenance),
        },
        Err(err) => Err(mask_failure::<E>(
            field,
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
    message = "this posture cannot mask a reply of type `{O}` on this subject",
    label = "the reply must serialize and deserialize, and this subject be an `#[expose]`d entity or a `#[subject]`",
    note = "a posture masks its reply against its entity's `#[expose]`d wire model, read and written as the entity's rows: expose the entity and answer a value that serializes and deserializes, or declare a capability no table holds with `#[subject]`, whose posture masks nothing"
)]
pub trait GraphqlMask<A, O> {
    /// The response mask's answer on an entity, for the field `site` declares;
    /// the value itself otherwise.
    fn masked_value(ctx: &Context<'_>, site: &'static Site, value: O) -> Result<O, Error>;

    /// [`masked_item_for`]'s answer on an entity; the item itself otherwise.
    fn masked_item(
        ctx: &Context<'_>,
        site: &'static Site,
        ability: &Ability,
        item: O,
    ) -> Result<Option<O>, Error>;
}

#[diagnostic::do_not_recommend]
impl<A, E, O> GraphqlMask<A, O> for Rows<E>
where
    A: ActionMarker,
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    fn masked_value(ctx: &Context<'_>, site: &'static Site, value: O) -> Result<O, Error> {
        masked_value_for::<A, E, O>(ctx, site, value)
    }

    fn masked_item(
        ctx: &Context<'_>,
        site: &'static Site,
        ability: &Ability,
        item: O,
    ) -> Result<Option<O>, Error> {
        masked_item_on::<E, O>(MaskedField::of(ctx, Some(site)), A::ACTION, ability, item)
    }
}

impl<A, O> GraphqlMask<A, O> for NoRows {
    fn masked_value(_: &Context<'_>, _: &'static Site, value: O) -> Result<O, Error> {
        Ok(value)
    }

    fn masked_item(
        _: &Context<'_>,
        _: &'static Site,
        _: &Ability,
        item: O,
    ) -> Result<Option<O>, Error> {
        Ok(Some(item))
    }
}

/// The value the `#[authorize(A, S)]` operation `site` declares answers, masked as
/// `S` masks it, what it refuses naming that operation.
pub fn graphql_masked_value<A, S, O>(
    ctx: &Context<'_>,
    site: &'static Site,
    value: O,
) -> Result<O, Error>
where
    S: SubjectMask,
    S::Mask: GraphqlMask<A, O>,
{
    <S::Mask as GraphqlMask<A, O>>::masked_value(ctx, site, value)
}

/// One subscription item the `#[authorize(A, S)]` operation `site` declares
/// streams, masked as `S` masks it through the ability its opening's gate checked.
pub fn graphql_masked_item<A, S, O>(
    ctx: &Context<'_>,
    site: &'static Site,
    ability: &Ability,
    item: O,
) -> Result<Option<O>, Error>
where
    S: SubjectMask,
    S::Mask: GraphqlMask<A, O>,
{
    <S::Mask as GraphqlMask<A, O>>::masked_item(ctx, site, ability, item)
}

/// The masked value did not fit the operation's own type: decide by selection
/// set (see [`masked_value_for`]).
fn unrepresentable<E, O>(
    field: MaskedField<'_, '_>,
    ability: &Ability,
    action: Action,
    value: O,
    provenance: Provenance,
) -> Result<O, Error>
where
    E: WireModel,
    O: Serialize + DeserializeOwned,
{
    // Written again rather than kept: the mask consumed the first, and only a
    // field grant the type cannot express comes this way.
    let wire = written::<E, O>(field, action, &value)?;
    let detail = mask_wire_detail::<E>(ability, action, wire, provenance).map_err(|err| {
        mask_failure::<E>(
            field,
            action,
            mask_reason::IRRECONCILABLE,
            "value could not be reconciled with the subject model",
            &err,
        )
    })?;

    if let Some(denial) = refused_field::<E>(field, action, &detail.removed) {
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
            field,
            action,
            mask_reason::IRRECONCILABLE,
            "masked value did not match the authorized subject type",
            &err,
        )
    })
}

/// The field a mask answers for: the context its selection and its operation's record
/// are read off, and the unit its lines name.
#[derive(Clone, Copy)]
struct MaskedField<'c, 'q> {
    ctx: &'c Context<'q>,
    unit: MaskedUnit<'c>,
}

impl<'c, 'q> MaskedField<'c, 'q> {
    /// The field `ctx` resolves: the one `site` declares, or, on a relation `#[expose]`
    /// emits, no site of its own.
    fn of(ctx: &'c Context<'q>, site: Option<&'static Site>) -> Self {
        let served = served_field(ctx, site);
        Self {
            ctx,
            unit: MaskedUnit::on(Edge::Graphql, site.map(Site::handler), served),
        }
    }

    /// Whether this field has yet to file `fact` in its operation. A line names its unit,
    /// so each unit files its own, once however many parents repeat it.
    fn first_to_file(self, fact: std::fmt::Arguments<'_>) -> bool {
        let MaskedUnit { site, served, .. } = self.unit;
        first_in_operation(self.ctx, format!("{site:?} {:?} {fact}", served.field))
    }
}

/// What a value serializes to, or the fail-closed exit of one that does not.
fn written<E: EntityTrait, O: Serialize>(
    field: MaskedField<'_, '_>,
    action: Action,
    value: &O,
) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|err| {
        mask_failure::<E>(
            field,
            action,
            mask_reason::NOT_SERIALIZABLE,
            "value did not serialize",
            &err,
        )
    })
}

/// The fields a field selects, fragments read through.
fn selection<'c>(ctx: &'c Context<'_>) -> Vec<&'c str> {
    ctx.field().selection_set().map(|f| f.name()).collect()
}

/// Whether the keys the mask stripped refuse `field`: those it selects, or every one
/// where it selects nothing, a scalar, which serializes whole.
fn refused_field<E: EntityTrait>(
    field: MaskedField<'_, '_>,
    action: Action,
    removed: &BTreeSet<String>,
) -> Option<Error> {
    let selected = selection(field.ctx);
    if selected.is_empty() {
        let whole: Vec<&str> = removed.iter().map(String::as_str).collect();
        return refused_selection::<E>(field, &whole, action, removed);
    }
    refused_selection::<E>(field, &selected, action, removed)
}

/// What a selection reads of each row of a connection: the fields its `nodes` and
/// its edges' `node` select, every exposed column where those select nothing, a
/// scalar, which serializes whole, and the primary key when it asks a cursor, whose
/// text is the row's key on a list relation's page, as the schema spells that field
/// though no selection named it.
fn page_selection<'c, E: WireModel>(ctx: &'c Context<'_>) -> Vec<&'c str> {
    let mut selected = Vec::new();
    let mut cursor = false;
    let node_selection = |node: SelectionField<'c>, selected: &mut Vec<&'c str>| {
        let before = selected.len();
        selected.extend(node.selection_set().map(|f| f.name()));
        if selected.len() == before {
            selected.extend(E::WIRE_KEYS);
        }
    };
    for field in ctx.field().selection_set() {
        match field.name() {
            "nodes" => node_selection(field, &mut selected),
            "edges" => {
                for edge in field.selection_set() {
                    match edge.name() {
                        "node" => node_selection(edge, &mut selected),
                        "cursor" => cursor = true,
                        _ => {}
                    }
                }
            }
            "pageInfo" => {
                cursor |= field
                    .selection_set()
                    .any(|f| matches!(f.name(), "startCursor" | "endCursor"));
            }
            _ => {}
        }
    }
    if cursor {
        selected.extend(
            E::PrimaryKey::iter()
                .filter_map(|key| E::wire_key(key.into_column()))
                .map(|key| schema_field_spelling(ctx, key)),
        );
    }
    selected
}

/// Whether the keys the mask stripped refuse the *field* that `selected` is read
/// off, each named to the client as `selected` spells it, and on the line by its key,
/// which every edge's line files.
///
/// A key outside the selection set of an object is never serialized, so it cannot
/// leak; a field with no selection is [`refused_field`]'s to read.
fn refused_selection<E: EntityTrait>(
    field: MaskedField<'_, '_>,
    selected: &[&str],
    action: Action,
    removed: &BTreeSet<String>,
) -> Option<Error> {
    let (keys, names): (Vec<&str>, Vec<String>) = removed
        .iter()
        .filter_map(|key| {
            let name = selected.iter().find(|name| same_key(name, key))?;
            Some((key.as_str(), (*name).to_owned()))
        })
        .unzip();
    if keys.is_empty() {
        return None;
    }
    // A tracing field must be a scalar, so the log keeps the joined form; the
    // wire keeps the list.
    let joined = keys.join(",");
    let subject = E::default().table_name();
    if field.first_to_file(format_args!("{subject} {action:?} {joined}")) {
        crate::gate::warn_denied(Refusal {
            subject: Some(subject),
            action: Some(action),
            fields: Some(&joined),
            reason: Some(reason::FIELD_NOT_GRANTED),
            ..Refusal::of_mask(field.unit)
        });
    }
    Some(forbidden_fields(&names))
}

/// One shape for every fail-closed masking exit: the queryable `warn`, one per field of
/// an operation however many parents repeat it, plus the opaque client error, which never
/// names the column or the reason.
fn mask_failure<E: EntityTrait>(
    field: MaskedField<'_, '_>,
    action: Action,
    reason: &'static str,
    detail: &'static str,
    err: &serde_json::Error,
) -> Error {
    let subject = E::default().table_name();
    if field.first_to_file(format_args!("{subject} {action:?} {reason}")) {
        warn_mask_failure(subject, action, reason, detail, field.unit, Some(err));
    }
    nest_rs_graphql::denial_to_graphql_error(Denial::internal("response masking failed"))
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
