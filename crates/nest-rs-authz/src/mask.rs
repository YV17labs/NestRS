//! Ambient (context-free) field-level masking.
//!
//! The transport bindings ([`crate::graphql::masked_output_for`],
//! [`crate::http::mask_entity_response`]) read the [`Ability`](crate::Ability)
//! from a transport handle. Some paths have none: a `#[dataloader]` batch runs
//! on a task async-graphql spawned off-request, and an MCP tool emits arbitrary
//! JSON-RPC content. Those read the ability from the ambient task-local instead
//! ([`current_ability`]), which [`crate::with_ability`] installs around the
//! batch / handler.

use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{ActionMarker, current_ability};

/// Mask `model` into the wire DTO `O` for action `A` using the ambient
/// [`Ability`](crate::Ability). Fails **closed**: with no ambient ability the
/// masked value is an empty object, so only unrestricted fields survive — a
/// wire type with required restricted fields errors rather than leaking a
/// fully-populated row.
pub fn masked_output_ambient<A, E, O>(model: &E::Model) -> Result<O, serde_json::Error>
where
    A: ActionMarker,
    E: EntityTrait,
    E::Model: Serialize,
    O: DeserializeOwned,
{
    let masked = match current_ability() {
        Some(ability) => ability.mask::<E>(A::ACTION, model),
        None => serde_json::Value::Object(Default::default()),
    };
    serde_json::from_value(masked)
}
