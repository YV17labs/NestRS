//! [`authorize`] — the class-level access gate, the WS analog of
//! [`crate::mcp::authorize`].

use nest_rs_guards::{Denial, denial_to_ws_error};
use nest_rs_ws::WsError;

use crate::gate::{Refusal, reason, transport};
use crate::{ActionMarker, GateVerdict, Subject, current_ability, gate};

/// Class-level gate: require action `A` on subject `S`, against the **ambient**
/// ability the connection's `SocketContext` re-installed for this message.
///
/// A missing ability is a wiring failure (no data context registered) and fails
/// **closed**: the upgrade's task-locals have unwound by the time a message arrives.
pub fn authorize<A: ActionMarker, S: Subject>(event: &'static str) -> Result<(), WsError> {
    let Some(ability) = current_ability() else {
        // A wiring failure: said to the operator, terse to the client.
        tracing::error!(
            target: crate::TARGET,
            transport = transport::WS,
            event = %event,
            action = ?A::ACTION,
            subject = std::any::type_name::<S>(),
            reason = reason::NO_AMBIENT_ABILITY,
            "authorization denied",
        );
        return Err(denial_to_ws_error(Denial::internal(
            "missing ambient `Ability` — is the WS data context registered as \
             `dyn SocketContext`?",
        )));
    };

    let verdict = gate::<A, S>(&ability);
    let denial = match &verdict {
        GateVerdict::Allowed => return Ok(()),
        GateVerdict::Unauthenticated => Denial::unauthorized("unauthenticated"),
        GateVerdict::Forbidden => Denial::forbidden("forbidden"),
        GateVerdict::InsufficientScope(missing) => {
            Denial::insufficient_scope(missing.clone(), "insufficient scope")
        }
    };
    crate::gate::warn_denied(Refusal {
        event: Some(event),
        reason: verdict.reason(),
        ..Refusal::of::<A, S>(transport::WS)
    });
    Err(denial_to_ws_error(denial))
}
