//! Ambient (context-free) field-level masking, for paths with no transport
//! handle — a `#[dataloader]` batch, an MCP tool's JSON-RPC content — which
//! read the ability from [`current_ability`].

use crate::ability::mask_reason;
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
        None => {
            crate::ability::warn_mask_failure(
                std::any::type_name::<E>(),
                A::ACTION,
                mask_reason::NO_AMBIENT_ABILITY,
                "no ambient ability — is the transport's authz bridge installed?",
                None,
                None,
                None,
            );
            serde_json::Value::Object(Default::default())
        }
    };
    serde_json::from_value(masked)
}
