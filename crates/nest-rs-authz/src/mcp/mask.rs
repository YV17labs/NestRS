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

/// What the developer changes so this refusal has a representation instead of
/// ending the operation. Prose, and it rides in the event's `remedy` field —
/// never as the `reason`, which is the value an incident groups by.
const NULLABLE_REMEDY: &str = "make the column `Option` on the entity, so a field grant's refusal has a \
     representation in the operation's return type";

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
/// requires **refuses the operation** ([`refused_fields`]), in the gate's own
/// vocabulary and no more informatively than a gate refusal.
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
/// It is a **denial**, and it is filed as one — the same event, the same
/// message and the same `reason` GraphQL files when a stripped key was
/// selected, because it is the same decision reached without a selection set to
/// weigh it against. Filed under the masking-failure event beside it instead,
/// as it was, the line answered "what went wrong in the mask" while the
/// question an incident asks is "what was this principal refused" — so a query
/// by `reason` returned GraphQL's refusals and missed these.
///
/// The detail pass runs only here, on the refusal, and buys the one thing the
/// serde error cannot say: **which** keys went missing.
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
        // Unreachable short of a `Serialize`/`Deserialize` disagreement: the
        // same value reconciled with the model a moment ago. Fail closed and
        // report it as the masking failure it then is, rather than claiming a
        // field grant refused something.
        Err(err) => {
            return mask_failure::<E>(
                action,
                mask_reason::IRRECONCILABLE,
                "value could not be reconciled with the subject model",
                Some(&err),
            );
        }
    };
    // **Every field removed is not a field grant refusing some — it is the class
    // refusing all.** `Ability::mask` does not consult `can`, so a row the
    // caller may not read at all yields `FieldSet::Only(∅)`, an empty object,
    // and the very same `from_value` failure a genuine field grant produces.
    // Filed as `field_not_granted` it listed every key and carried the nullable
    // remedy — advice that, if taken, would answer a denied row with an
    // all-null object rather than a refusal. The two are one code path and two
    // decisions, so the report names which.
    let whole_subject = declared > 0 && removed.len() == declared;
    let fields = removed.into_iter().collect::<Vec<_>>().join(",");
    crate::gate::warn_denied(Refusal {
        subject: Some(std::any::type_name::<E>()),
        action: Some(action),
        // The keys still travel joined — a tracing field must be a scalar — but
        // only where they are the answer. On a class denial they are every key
        // there is, which says nothing the subject did not.
        fields: (!whole_subject).then_some(&fields),
        reason: Some(if whole_subject {
            reason::NO_CLASS_GRANT
        } else {
            reason::FIELD_NOT_GRANTED
        }),
        remedy: (!whole_subject).then_some(NULLABLE_REMEDY),
        ..Refusal::on(transport::MCP)
    });
    // The gate's own vocabulary, through the same conversion: a caller refused
    // by the mask and one refused by the gate learn the same thing, which is
    // all either may learn.
    denial_to_mcp_error(Denial::forbidden("forbidden"))
}

/// One shape for every fail-closed masking exit: the queryable `warn` (so a
/// branch that forgets it is the visible omission) plus an error that names
/// neither the column nor the reason — the reader is a language model.
fn mask_failure<E>(
    action: Action,
    reason: &'static str,
    detail: &'static str,
    err: Option<&serde_json::Error>,
) -> McpError
where
    E: EntityTrait,
{
    // One delegation, both arms: the `None` case used to hand-copy the event
    // beside the shared emitter, which is the duplicate `warn_mask_failure`'s
    // own doc forbids.
    warn_mask_failure(
        std::any::type_name::<E>(),
        action,
        reason,
        detail,
        Some(transport::MCP),
        None,
        err.map(|e| e as &dyn std::fmt::Display),
    );
    denial_to_mcp_error(Denial::internal("response masking failed"))
}
