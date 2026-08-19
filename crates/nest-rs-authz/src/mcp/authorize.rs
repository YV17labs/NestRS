//! [`authorize`] — the class-level access gate, the MCP analog of
//! [`crate::graphql::authorize`].

use nest_rs_guards::{Denial, denial_to_mcp_error};
use nest_rs_mcp::McpError;

use crate::gate::{Refusal, reason, transport};
use crate::{ActionMarker, GateVerdict, Subject, current_ability, gate};

/// Class-level gate: require action `A` on subject `S`, against the **ambient**
/// ability the endpoint's operation guard installed.
///
/// The decision itself is [`gate`], shared with GraphQL so `#[authorize]` cannot
/// come to mean two things; this function is the MCP half — where the ability
/// comes from, and what a refusal looks like on the wire.
///
/// A missing ability is a wiring failure (no bridge registered) and fails
/// **closed** rather than reading as unrestricted.
pub fn authorize<A: ActionMarker, S: Subject>() -> Result<(), McpError> {
    let Some(ability) = current_ability() else {
        // Not a client error: the operation guard did not install an ability, so
        // the gate has nothing to decide against. Say so to the operator, stay
        // opaque to the model.
        tracing::error!(
            target: crate::TARGET,
            transport = transport::MCP,
            action = ?A::ACTION,
            subject = std::any::type_name::<S>(),
            reason = reason::NO_AMBIENT_ABILITY,
            "authorization denied",
        );
        return Err(denial_to_mcp_error(Denial::internal(
            "missing ambient `Ability` — is the MCP authz bridge registered as \
             `dyn McpOperationGuard`?",
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
        reason: verdict.reason(),
        ..Refusal::of::<A, S>(transport::MCP)
    });
    Err(denial_to_mcp_error(denial))
}
