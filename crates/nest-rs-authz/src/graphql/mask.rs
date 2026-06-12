//! Field-level response masking for GraphQL resolvers — the transport analog of
//! [`crate::http::mask_entity_response`].
//!
//! The framework-carried path is [`masked_value_for`], emitted automatically by
//! `#[resolver]` after every `#[authorize(Action, Entity)]`-declared operation —
//! a hand-written resolver never calls it. [`masked_output`] /
//! [`masked_output_for`] remain the manual primitives for custom shapes the
//! wrapper cannot see through (e.g. a cursor connection), paired with
//! `#[authorize(…, unmasked)]`.

use nest_rs_graphql::async_graphql::{Context, Error};
use nest_rs_resource::WireModelDefaults;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::ability;
use crate::wire_mask::{MaskedWire, mask_wire_json};
use crate::{Ability, Action, ActionMarker};

/// Mask a resolver's already-built wire value through the ambient ability —
/// the GraphQL analog of the HTTP response shaper, sharing its value-level
/// round-trip (`crate::wire_mask`): serialize the value, reconstruct each
/// object into `E::Model` (filling unexposed columns via
/// [`WireModelDefaults`]), run [`Ability::mask`] / [`Ability::mask_many`],
/// retain only the exposed wire keys, deserialize back. Handles the wire DTO
/// itself, `Option<…>`, `Vec<…>`; scalars and `None` pass through untouched.
///
/// Fails **closed**: an irreconcilable value is a GraphQL error, never
/// unmasked data. Same caveat as HTTP masking: a hidden column an ability rule
/// predicates on is reconstructed from its [`WireModelDefaults`] default, so
/// such columns are best left exposed.
pub fn masked_value_for<A, E, O>(ctx: &Context<'_>, value: O) -> Result<O, Error>
where
    A: ActionMarker,
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
    O: Serialize + DeserializeOwned,
{
    let ability = ability(ctx)?;
    let wire = serde_json::to_value(&value)
        .map_err(|err| Error::new(format!("response masking failed: {err}")))?;
    match mask_wire_json::<E>(&ability, A::ACTION, &wire) {
        Ok(MaskedWire::Passthrough) => Ok(value),
        Ok(MaskedWire::Masked(masked)) => serde_json::from_value(masked).map_err(|_| {
            Error::new("response masking failed: value did not match the authorized subject type")
        }),
        Err(_) => Err(Error::new(
            "response masking failed: value did not match the authorized subject type",
        )),
    }
}

/// Mask a loaded model into the wire output type using the ambient ability.
pub fn masked_output<E, O>(ability: &Ability, action: Action, model: &E::Model) -> Result<O, Error>
where
    E: EntityTrait,
    E::Model: Serialize,
    O: DeserializeOwned,
{
    let masked = ability.mask::<E>(action, model);
    serde_json::from_value(masked)
        .map_err(|err| Error::new(format!("response masking failed: {err}")))
}

/// Read the ambient ability and mask `model` into `O`.
pub fn masked_output_for<A, E, O>(
    ctx: &nest_rs_graphql::async_graphql::Context<'_>,
    model: &E::Model,
) -> Result<O, Error>
where
    A: ActionMarker,
    E: EntityTrait,
    E::Model: Serialize,
    O: DeserializeOwned,
{
    let ability = ability(ctx)?;
    masked_output::<E, O>(&ability, A::ACTION, model)
}
