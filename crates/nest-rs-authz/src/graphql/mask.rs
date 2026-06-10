//! Field-level response masking for GraphQL resolvers — the transport analog of
//! [`crate::http::mask_entity_response`].

use nest_rs_graphql::async_graphql::Error;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::ability;
use crate::{Ability, Action, ActionMarker};

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
