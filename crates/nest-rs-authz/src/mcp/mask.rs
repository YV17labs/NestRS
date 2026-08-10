//! Field-level response masking for MCP operations — the transport analog of
//! [`crate::graphql::masked_value_for`].
//!
//! The framework-carried path is [`masked_value_for`], emitted automatically by
//! `#[tools]` after every `#[authorize(Action, Entity)]`-declared operation; a
//! host never calls it. [`crate::masked_output_ambient`] remains the manual
//! primitive for a hand-written `ServerHandler` surface, which no decorator
//! reaches.

use nest_rs_guards::{Denial, denial_to_mcp_error};
use nest_rs_mcp::McpError;
use nest_rs_resource::WireModelDefaults;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::wire_mask::{MaskedWire, mask_wire_json, warn_mask_failure};
use crate::{Action, ActionMarker, current_ability};

/// Mask an operation's already-built value through the ambient ability, sharing
/// the value-level round-trip every transport masks with
/// (`crate::wire_mask`): serialize, reconstruct each object into `E::Model`
/// (filling unexposed columns via [`WireModelDefaults`]), run
/// [`Ability::mask`](crate::Ability::mask) /
/// [`Ability::mask_many`](crate::Ability::mask_many), retain only the exposed
/// wire keys, deserialize back. Rows the ability refuses are dropped; scalars
/// and `null` pass through untouched — which is why a tool returning a `String`
/// summary is masked by this call and unaffected by it.
///
/// # A stripped field the return type cannot express
///
/// MCP has no selection set. GraphQL can ask "did the operation *ask* for the
/// column the mask removed?" and serve the value when it did not; HTTP can drop
/// the key from a JSON body. An MCP operation returns one fixed Rust type to a
/// model, so neither escape exists — a mask that takes a key the return type
/// requires **fails the operation**, opaquely.
///
/// That is the fail-closed reading, and it has a remedy the host controls: a
/// column an ability rule may mask should be `Option` on the entity, exactly as
/// on GraphQL. The alternative — serving the row because the shape could not
/// express the refusal — is the leak this whole path exists to prevent.
pub fn masked_value_for<A, E, O>(value: O) -> Result<O, McpError>
where
    A: ActionMarker,
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
    O: Serialize + DeserializeOwned,
{
    let action = A::ACTION;
    let Some(ability) = current_ability() else {
        // Fail closed: no ability means nothing decided what this caller may
        // read, and shipping the unmasked value would answer that question with
        // "everything".
        return Err(mask_failure::<E>(
            action,
            "no ambient ability — is the MCP authz bridge registered?",
            None,
        ));
    };
    let wire = match serde_json::to_value(&value) {
        Ok(wire) => wire,
        Err(err) => {
            return Err(mask_failure::<E>(
                action,
                "operation value did not serialize",
                Some(&err),
            ));
        }
    };
    match mask_wire_json::<E>(&ability, action, &wire) {
        Ok(MaskedWire::Passthrough) => Ok(value),
        Ok(MaskedWire::Masked(masked)) => serde_json::from_value(masked).map_err(|err| {
            // The mask took a key the return type requires. Unlike GraphQL there
            // is no selection set to acquit it with, so this is the refusal.
            mask_failure::<E>(
                action,
                "masked value did not fit the operation's return type — a column a \
                 field grant may mask must be nullable on the entity",
                Some(&err),
            )
        }),
        Err(err) => Err(mask_failure::<E>(
            action,
            "value could not be reconciled with the subject model",
            Some(&err),
        )),
    }
}

/// One shape for every fail-closed masking exit: the queryable `warn` (so a
/// branch that forgets it is the visible omission) plus an error that names
/// neither the column nor the reason — the reader is a language model.
fn mask_failure<E>(
    action: Action,
    reason: &'static str,
    err: Option<&serde_json::Error>,
) -> McpError
where
    E: EntityTrait,
{
    match err {
        Some(err) => warn_mask_failure(std::any::type_name::<E>(), action, reason, err),
        None => tracing::warn!(
            target: "nest_rs::authz",
            entity = std::any::type_name::<E>(),
            action = ?action,
            reason,
            "response masking failed",
        ),
    }
    denial_to_mcp_error(Denial::internal("response masking failed"))
}
