//! The class-level access decision, written once: authentication first, then
//! the class grant, then the refusal a wider token would have fixed. Each
//! transport maps the verdict to its own error.

#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
use std::any::TypeId;

#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
use crate::Ability;
use crate::{Action, ActionMarker, Subject};

/// What the class-level gate decided.
#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
pub enum GateVerdict {
    /// The caller holds the class grant.
    Allowed,
    /// No principal backs the request. Refused for want of authentication
    /// whatever the visitor branch granted — a grant written to serve a
    /// `#[public]` operation must not satisfy an `#[authorize]` one.
    Unauthenticated,
    /// A principal, but no grant. Final: a wider token would not help.
    Forbidden,
    /// A principal whose *credential* lacks a scope the rule requires, naming
    /// the scopes to ask the authorization server for. Actionable, which is why
    /// RFC 6750 §3.1 keeps it apart from [`Forbidden`](Self::Forbidden).
    InsufficientScope(Vec<String>),
}

/// The machine-readable denial reasons, spelled once.
pub(crate) mod reason {
    /// No principal at all — the gate's first rung.
    #[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
    pub(crate) const ANONYMOUS_CALLER: &str = "anonymous_caller";
    /// A principal with no grant on the subject class.
    pub(crate) const NO_CLASS_GRANT: &str = "no_class_grant";
    /// A principal whose token is too narrow — RFC 6750 §3.1.
    pub(crate) const INSUFFICIENT_SCOPE: &str = "insufficient_scope";
    /// Nothing installed an ability: a wiring failure, reported by every
    /// fail-closed exit, masking included (via [`mask_reason`](crate::ability::mask_reason)).
    #[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
    pub(crate) const NO_AMBIENT_ABILITY: &str = crate::ability::mask_reason::NO_AMBIENT_ABILITY;
    /// A field grant stripped a key the answer cannot be delivered without.
    /// Only the edges handing a typed value back reach it; HTTP and WS drop the key.
    #[cfg(any(feature = "graphql", feature = "mcp"))]
    pub(crate) const FIELD_NOT_GRANTED: &str = "field_not_granted";
}

/// The edge a refusal was filed on, spelled once per edge.
pub(crate) mod transport {
    /// The HTTP edge — a route's `Authorize<A, E>` shaper.
    #[cfg(feature = "http")]
    pub(crate) const HTTP: &str = "http";
    /// The GraphQL edge — a resolver operation or a federation root field.
    #[cfg(feature = "graphql")]
    pub(crate) const GRAPHQL: &str = "graphql";
    /// The WebSocket edge — one message on an established connection.
    #[cfg(feature = "ws")]
    pub(crate) const WS: &str = "ws";
    /// The MCP edge — one tool call or prompt fetch.
    #[cfg(feature = "mcp")]
    pub(crate) const MCP: &str = "mcp";
}

#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
impl GateVerdict {
    /// The machine-readable reason a denial logs and reports, or `None` when
    /// nothing was denied.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Allowed => None,
            Self::Unauthenticated => Some(reason::ANONYMOUS_CALLER),
            Self::Forbidden => Some(reason::NO_CLASS_GRANT),
            Self::InsufficientScope(_) => Some(reason::INSUFFICIENT_SCOPE),
        }
    }
}

/// Everything the one `authorization denied` event can carry, and the only
/// place any of its fields is named.
#[derive(Clone, Copy)]
pub(crate) struct Refusal<'a> {
    /// The edge that refused — a [`transport`] constant.
    pub transport: &'static str,
    /// The edge's own name for the unit of work, where it has one beside the
    /// subject: a WS event name. GraphQL and MCP report the operation on the
    /// chain's own line instead.
    pub event: Option<&'a str>,
    /// The action `#[authorize]` declared, where a posture declared one.
    pub action: Option<Action>,
    /// The subject class reached for.
    pub subject: Option<&'static str>,
    /// The wire keys a field grant stripped, joined — `tracing` records
    /// scalars, so the list is flattened here and kept structured on the wire.
    pub fields: Option<&'a str>,
    /// The machine-readable reason, from [`reason`]. `None` only where a
    /// verdict reports none, which is the allowed case nobody logs.
    pub reason: Option<&'static str>,
    /// What the developer would change — prose, never the `reason` value.
    pub remedy: Option<&'static str>,
}

impl<'a> Refusal<'a> {
    /// A refusal on `transport`, with nothing else stated yet.
    pub(crate) fn on(transport: &'static str) -> Self {
        Self {
            transport,
            event: None,
            action: None,
            subject: None,
            fields: None,
            reason: None,
            remedy: None,
        }
    }

    /// A refusal by the class gate or the response mask, which always name
    /// what `#[authorize(Action, Entity)]` declared.
    pub(crate) fn of<A: ActionMarker, S: Subject>(transport: &'static str) -> Self {
        Self {
            action: Some(A::ACTION),
            subject: Some(std::any::type_name::<S>()),
            ..Self::on(transport)
        }
    }
}

/// The one `warn` every authorization refusal passes through, so a denial
/// cannot reach a client without leaving the queryable trace an incident is
/// answered from.
pub(crate) fn warn_denied(refusal: Refusal<'_>) {
    let Refusal {
        transport,
        event,
        action,
        subject,
        fields,
        reason,
        remedy,
    } = refusal;
    tracing::warn!(
        target: crate::TARGET,
        transport,
        event,
        // `Option<Action>` is not a `tracing` value; `field::debug` lets it be absent.
        action = action.map(tracing::field::debug),
        subject,
        fields,
        reason,
        remedy,
        "authorization denied",
    );
}

/// Decide action `A` on subject `S` against `ability`.
#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
pub fn gate<A: ActionMarker, S: Subject>(ability: &Ability) -> GateVerdict {
    if ability.is_visitor() {
        return GateVerdict::Unauthenticated;
    }
    if ability.can_class(A::ACTION, TypeId::of::<S>()) {
        return GateVerdict::Allowed;
    }
    let missing = ability.missing_scopes(A::ACTION, TypeId::of::<S>());
    if missing.is_empty() {
        return GateVerdict::Forbidden;
    }
    GateVerdict::InsufficientScope(missing)
}
