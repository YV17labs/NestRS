//! The class-level access decision, written once.
//!
//! `#[authorize(Action, Entity)]` means the same thing on every transport, and
//! that is a claim about *this* ladder: authentication first and separately,
//! then the class grant, then — distinctly — the refusal a wider token would
//! have fixed. Only the error each transport renders differs, so it takes the
//! shape [`run_ability_chain`](crate::run_ability_chain) already set: the
//! ordering lives here and the caller maps the verdict.
//!
//! Written as a verdict rather than a `Result` because the three refusals are
//! not one error with three messages — a client branches on them, and each
//! transport spells that branching in its own vocabulary.

#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
use std::any::TypeId;

#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
use crate::Ability;
use crate::{ActionMarker, Subject};

/// What the class-level gate decided.
///
/// The verdict and [`gate`] belong to the three transports that decide **in
/// band**. HTTP's gate is the `Authorize<A, E>` extractor, which reaches only
/// `warn_denied` here — so an `http`-only build compiles this module for that
/// one function and would carry the rest as dead code. (Named without a link:
/// the module is private, and an intra-doc link to it renders dead on docs.rs
/// — a warning only `cargo doc` sees, which no gate in this repo runs.)
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

#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
impl GateVerdict {
    /// The machine-readable reason a denial logs and reports, or `None` when
    /// nothing was denied.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Allowed => None,
            Self::Unauthenticated => Some("anonymous_caller"),
            Self::Forbidden => Some("no_class_grant"),
            Self::InsufficientScope(_) => Some("insufficient_scope"),
        }
    }
}

/// The one `warn` every gate refusal passes through, so a denial cannot reach a
/// client without leaving the queryable trace an incident is answered from.
///
/// Beside [`gate`] rather than in each transport, because unlike the `Opaque`
/// seams — whose messages are deliberately different per edge — the target and
/// the event name here are the *same literal* on all three, and only fields
/// vary. A field added for one transport and missed on the other two is exactly
/// what having three copies buys.
///
/// `event` is `None` where the transport has no per-operation name of its own to
/// report beyond the subject (GraphQL, MCP).
pub fn warn_denied<A: ActionMarker, S: Subject>(
    transport: &'static str,
    event: Option<&str>,
    reason: Option<&'static str>,
) {
    tracing::warn!(
        target: crate::TARGET,
        transport,
        event,
        action = ?A::ACTION,
        subject = std::any::type_name::<S>(),
        reason,
        "authorization denied",
    );
}

/// Decide action `A` on subject `S` against `ability`.
#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
pub fn gate<A: ActionMarker, S: Subject>(ability: &Ability) -> GateVerdict {
    // Authentication first, and separately: an anonymous caller is refused for
    // want of a principal, whatever the visitor branch granted.
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
