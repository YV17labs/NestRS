//! Field-level masking for WS message replies.
//!
//! The framework-carried path is [`masked_reply_for`], emitted automatically by
//! `#[messages]` after every `#[authorize(Action, Entity)]`-declared message; a
//! gateway never calls it. [`crate::masked_reply`] masks a value no decorator
//! reaches, for the caller whose ability is ambient — never per recipient of a
//! `WsServer::emit`.
//!
//! Unlike GraphQL and MCP, this ships the masked serialized value rather than
//! reconstructing the handler's type: a WS frame, like an HTTP body, has no
//! schema promising a key, so a stripped key is simply absent.

use nest_rs_core::Edge;
use nest_rs_guards::Denial;
use nest_rs_ws::WsError;
use nest_rs_ws::denial_to_ws_error;
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::ability::{Provenance, mask_reason};
use crate::wire_mask::{MaskedWire, mask_wire_json, warn_mask_failure};
use crate::{Action, ActionMarker, WireModelDefaults, current_ability};

/// Mask a message reply through the ambient ability and return the JSON the frame
/// should carry.
///
/// Rows the ability refuses are dropped, field grants strip columns, unexposed
/// columns are strained out; scalars and `null` pass through.
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
        // Fail closed: nothing decided what this caller may read.
        return Err(mask_failure::<E>(
            event,
            action,
            mask_reason::NO_AMBIENT_ABILITY,
            "no ambient ability — does the gateway's upgrade run an `AbilityGuard`?",
            None,
        ));
    };
    let wire = match serde_json::to_value(value) {
        Ok(wire) => wire,
        Err(err) => {
            return Err(mask_failure::<E>(
                event,
                action,
                mask_reason::NOT_SERIALIZABLE,
                "message value did not serialize",
                Some(&err),
            ));
        }
    };
    match mask_wire_json::<E>(&ability, action, &wire, Provenance::Loaded) {
        Ok(MaskedWire::Masked(masked)) => Ok(masked),
        Ok(MaskedWire::Passthrough) => Ok(wire),
        Err(err) => Err(mask_failure::<E>(
            event,
            action,
            mask_reason::IRRECONCILABLE,
            "value could not be reconciled with the subject model",
            Some(&err),
        )),
    }
}

/// One shape for every fail-closed masking exit: the queryable `warn` plus a
/// frame that names neither the column nor the reason.
fn mask_failure<E>(
    event: &'static str,
    action: Action,
    reason: &'static str,
    detail: &'static str,
    err: Option<&serde_json::Error>,
) -> WsError
where
    E: EntityTrait,
{
    warn_mask_failure(
        std::any::type_name::<E>(),
        action,
        reason,
        detail,
        Some(Edge::Ws.as_str()),
        Some(event),
        err.map(|e| e as &(dyn std::error::Error + 'static)),
    );
    denial_to_ws_error(Denial::internal("response masking failed"))
}
