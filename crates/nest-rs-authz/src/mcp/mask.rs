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

use crate::ability::mask_reason;
use crate::gate::{Refusal, reason, transport};
use crate::wire_mask::{MaskedWire, mask_wire_detail, mask_wire_json, warn_mask_failure};
use crate::{Ability, Action, ActionMarker, current_ability};

/// What the developer changes so this refusal has a representation; rides in `remedy`.
const NULLABLE_REMEDY: &str = "make the column `Option` on the entity, so a field grant's refusal has a \
     representation in the operation's return type";

/// Mask an operation's already-built value through the ambient ability, sharing
/// the value-level round-trip every transport masks with
/// (`crate::wire_mask`): serialize, reconstruct each object into `E::Model`
/// (filling unexposed columns via [`WireModelDefaults`]), run
/// [`Ability::mask`](crate::Ability::mask) /
/// [`Ability::mask_many`](crate::Ability::mask_many), retain only the exposed
/// wire keys, deserialize back. Rows the ability refuses are dropped; scalars
/// and `null` pass through untouched.
///
/// MCP has no selection set: a mask that takes a key the return type requires
/// **refuses the operation**. A column an ability rule may mask should be
/// `Option` on the entity.
pub fn masked_value_for<A, E, O>(value: O) -> Result<O, McpError>
where
    A: ActionMarker,
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
    O: Serialize + DeserializeOwned,
{
    let action = A::ACTION;
    let Some(ability) = current_ability() else {
        // Fail closed: nothing decided what this caller may read.
        return Err(mask_failure::<E>(
            action,
            mask_reason::NO_AMBIENT_ABILITY,
            "no ambient ability — is the MCP authz bridge registered?",
            None,
        ));
    };
    let wire = match serde_json::to_value(&value) {
        Ok(wire) => wire,
        Err(err) => {
            return Err(mask_failure::<E>(
                action,
                mask_reason::NOT_SERIALIZABLE,
                "operation value did not serialize",
                Some(&err),
            ));
        }
    };
    match mask_wire_json::<E>(&ability, action, &wire) {
        Ok(MaskedWire::Passthrough) => Ok(value),
        Ok(MaskedWire::Masked(masked)) => match serde_json::from_value(masked) {
            Ok(masked) => Ok(masked),
            // The mask took a key the return type requires. Unlike GraphQL there
            // is no selection set to acquit it with, so this is the refusal.
            Err(_) => Err(refused_fields::<E>(&ability, action, wire)),
        },
        Err(err) => Err(mask_failure::<E>(
            action,
            mask_reason::IRRECONCILABLE,
            "value could not be reconciled with the subject model",
            Some(&err),
        )),
    }
}

/// The refusal a field grant makes on a shape that cannot express it.
///
/// Filed as a denial: the same event and `reason` GraphQL files for a selected
/// stripped key.
fn refused_fields<E>(ability: &Ability, action: Action, wire: serde_json::Value) -> McpError
where
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
{
    // Counted before the mask consumes the value, to tell two refusals apart
    // below.
    let declared = wire.as_object().map_or(0, serde_json::Map::len);
    let removed = match mask_wire_detail::<E>(ability, action, wire) {
        Ok(detail) => detail.removed,
        // Unreachable short of a `Serialize`/`Deserialize` disagreement; reported
        // as the masking failure it then is.
        Err(err) => {
            return mask_failure::<E>(
                action,
                mask_reason::IRRECONCILABLE,
                "value could not be reconciled with the subject model",
                Some(&err),
            );
        }
    };
    // Every field removed is the class refusing the row (`Ability::mask` does not
    // consult `can`), not a field grant: no `fields`, no nullable remedy.
    let whole_subject = declared > 0 && removed.len() == declared;
    let fields = removed.into_iter().collect::<Vec<_>>().join(",");
    crate::gate::warn_denied(Refusal {
        subject: Some(std::any::type_name::<E>()),
        action: Some(action),
        // Joined, a tracing field being a scalar; omitted when it is every key.
        fields: (!whole_subject).then_some(&fields),
        reason: Some(if whole_subject {
            reason::NO_CLASS_GRANT
        } else {
            reason::FIELD_NOT_GRANTED
        }),
        remedy: (!whole_subject).then_some(NULLABLE_REMEDY),
        ..Refusal::on(transport::MCP)
    });
    // The gate's own vocabulary: a mask refusal tells no more than a gate's.
    denial_to_mcp_error(Denial::forbidden("forbidden"))
}

/// One shape for every fail-closed masking exit: the queryable `warn` plus an
/// error that names neither the column nor the reason — the reader is a model.
fn mask_failure<E>(
    action: Action,
    reason: &'static str,
    detail: &'static str,
    err: Option<&serde_json::Error>,
) -> McpError
where
    E: EntityTrait,
{
    warn_mask_failure(
        std::any::type_name::<E>(),
        action,
        reason,
        detail,
        Some(transport::MCP),
        None,
        err.map(|e| e as &(dyn std::error::Error + 'static)),
    );
    denial_to_mcp_error(Denial::internal("response masking failed"))
}
