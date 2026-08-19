//! [`authorize`] — the class-level access gate, the WS analog of
//! [`crate::mcp::authorize`].

use nest_rs_guards::{Denial, denial_to_ws_error};
use nest_rs_ws::WsError;

use crate::gate::{Refusal, reason, transport};
use crate::{ActionMarker, GateVerdict, Subject, current_ability, gate};

/// Class-level gate: require action `A` on subject `S`, against the **ambient**
/// ability the connection's `SocketContext` re-installed for this message.
///
/// The decision itself is [`gate`], shared with GraphQL and MCP so `#[authorize]`
/// cannot come to mean three things; this function is the WS half — where the
/// ability comes from, and what a refusal looks like on the wire.
///
/// A missing ability is a wiring failure (no data context registered) and fails
/// **closed** rather than reading as unrestricted. That is the same reading the
/// other two transports take, and on WS it is the one that matters most: the
/// upgrade's task-locals have already unwound by the time a message arrives, so
/// "no ability" is the *expected* state of a gateway whose `AuthzWsModule` was
/// never imported.
pub fn authorize<A: ActionMarker, S: Subject>(event: &'static str) -> Result<(), WsError> {
    let Some(ability) = current_ability() else {
        // Not a client error: nothing installed an ability, so the gate has
        // nothing to decide against. Say so to the operator, stay terse to the
        // client.
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
