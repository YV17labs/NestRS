//! Field-level masking for WS message replies.
//!
//! The framework-carried path is [`masked_reply_for`], emitted automatically by
//! `#[messages]` after every `#[authorize(Action, Entity)]`-declared message; a
//! gateway never calls it. [`crate::masked_reply`] remains the manual primitive
//! for a hand-built server push (`WsServer::emit`), which no decorator reaches.
//!
//! # Why this masks the serialized value, unlike MCP
//!
//! GraphQL and MCP both mask *and then reconstruct* the operation's return type,
//! so a mask that strips a required key fails the operation: GraphQL because the
//! schema declared the field non-nullable, MCP because rmcp needs the typed value
//! back for `structuredContent`.
//!
//! A WS reply has neither. The envelope carries whatever JSON the handler's value
//! serialized to, with no schema promising a key is present — which puts WS in
//! **HTTP's** position, not MCP's: the masked key is simply absent from the frame,
//! exactly as `RouteResponseShaper` omits it from a body. So this masks the
//! serialized value and ships it, rather than round-tripping back into the
//! handler's type and failing on shapes the mask can no longer inhabit.
//!
//! Two consequences, both wanted: the reply type needs no `DeserializeOwned` (so
//! a handler may answer with a borrowed or non-deserializable shape), and a
//! field-restricted caller reads a smaller object instead of an error. What stays
//! fail-closed is what should: no ambient ability, and a body that cannot be
//! reconciled with the entity at all.

use nest_rs_guards::{Denial, denial_to_ws_error};
use nest_rs_resource::WireModelDefaults;
use nest_rs_ws::WsError;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::wire_mask::{MaskedWire, mask_wire_json, warn_mask_failure};
use crate::{Action, ActionMarker, current_ability};

/// Mask a message reply through the ambient ability and return the JSON the frame
/// should carry.
///
/// Shares the value-level round-trip every transport masks with
/// (`crate::wire_mask`): serialize, reconstruct each object into `E::Model`
/// (filling unexposed columns via [`WireModelDefaults`]), run
/// [`Ability::mask`](crate::Ability::mask) /
/// [`Ability::mask_many`](crate::Ability::mask_many), then retain only the exposed
/// wire keys. Rows the ability refuses are dropped; field grants strip columns;
/// scalars and `null` pass through untouched — which is why a message answering
/// with a count is masked by this call and unaffected by it.
///
/// Fails closed on the two cases that are wiring or data faults rather than
/// policy: no ambient ability (nothing decided what this caller may read), and a
/// body that cannot be reconciled with `E::Model` at all.
pub fn masked_reply_for<A, E, O>(event: &'static str, value: &O) -> Result<Value, WsError>
where
    A: ActionMarker,
    E: EntityTrait + WireModelDefaults,
    E::Model: Serialize + DeserializeOwned,
    O: Serialize + ?Sized,
{
    let action = A::ACTION;
    let Some(ability) = current_ability() else {
        // Shipping the unmasked value here would answer "what may this caller
        // read?" with "everything".
        return Err(mask_failure::<E>(
            event,
            action,
            "no ambient ability — is the WS data context registered as \
             `dyn SocketContext`?",
            None,
        ));
    };
    let wire = match serde_json::to_value(value) {
        Ok(wire) => wire,
        Err(err) => {
            return Err(mask_failure::<E>(
                event,
                action,
                "message value did not serialize",
                Some(&err),
            ));
        }
    };
    match mask_wire_json::<E>(&ability, action, &wire) {
        Ok(MaskedWire::Masked(masked)) => Ok(masked),
        Ok(MaskedWire::Passthrough) => Ok(wire),
        Err(err) => Err(mask_failure::<E>(
            event,
            action,
            "value could not be reconciled with the subject model",
            Some(&err),
        )),
    }
}

/// One shape for every fail-closed masking exit: the queryable `warn` (so a
/// branch that forgets it is the visible omission) plus a frame that names
/// neither the column nor the reason.
fn mask_failure<E>(
    event: &'static str,
    action: Action,
    reason: &'static str,
    err: Option<&serde_json::Error>,
) -> WsError
where
    E: EntityTrait,
{
    match err {
        Some(err) => warn_mask_failure(std::any::type_name::<E>(), action, reason, err),
        None => tracing::warn!(
            target: crate::TARGET,
            transport = "ws",
            event = %event,
            entity = std::any::type_name::<E>(),
            action = ?action,
            reason,
            "response masking failed",
        ),
    }
    denial_to_ws_error(Denial::internal("response masking failed"))
}
