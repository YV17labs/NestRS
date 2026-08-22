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
use crate::{Action, ActionMarker, Subject};

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

/// The machine-readable denial reasons, spelled once.
///
/// Always compiled, unlike [`GateVerdict::reason`]: HTTP does not build a
/// verdict — argued on [`Ability::is_visitor`](crate::Ability::is_visitor) — so
/// in an `http`-only build the enum's accessor is `cfg`'d out and the transport
/// re-typed these as literals. Two spellings of one value space is what lets an
/// incident query match three edges and miss the fourth, and "every string the
/// framework interprets is a constant" is the rule that forbids it.
///
/// Each constant carries the `cfg` of the builds that can *reach* it, so an
/// unused one stays a `dead_code` warning rather than being blanket-allowed: a
/// value nothing files is a vocabulary entry nobody can query.
pub(crate) mod reason {
    /// No principal at all — the gate's first rung. Only a build with an
    /// in-band edge forms a verdict, and the verdict is what reports this.
    #[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
    pub const ANONYMOUS_CALLER: &str = "anonymous_caller";
    /// A principal with no grant on the subject class.
    pub const NO_CLASS_GRANT: &str = "no_class_grant";
    /// A principal whose token is too narrow — RFC 6750 §3.1.
    pub const INSUFFICIENT_SCOPE: &str = "insufficient_scope";
    /// Nothing installed an ability, so nothing decided what this caller may
    /// do. A wiring failure rather than a client one, and the reason every
    /// fail-closed exit reports — the guard's per-operation entries, the in-band
    /// gates, and the masking paths alike, the last of which reach it through
    /// [`mask_reason`](crate::ability::mask_reason) so an incident query on this
    /// one value finds all three.
    #[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
    pub const NO_AMBIENT_ABILITY: &str = crate::ability::mask_reason::NO_AMBIENT_ABILITY;
    /// A field grant stripped a key the answer cannot be delivered without.
    /// The refusal a *mask* makes rather than a gate, and the same decision on
    /// every edge that reaches it — which is why it is one value here and not
    /// one sentence per transport. The two edges that hand a **typed** value
    /// back are the ones that can reach it; HTTP drops the key from the body
    /// and WS from the frame, so neither refuses.
    #[cfg(any(feature = "graphql", feature = "mcp"))]
    pub const FIELD_NOT_GRANTED: &str = "field_not_granted";
}

/// The edge a refusal was filed on, spelled once per edge.
///
/// A field *value* an operator filters on, so it earns the same treatment as
/// [`reason`]: `transport = "grahpql"` selects nothing and says nothing, and
/// the guard entries name three edges from a single file, where a literal has
/// nowhere to be checked against.
pub(crate) mod transport {
    /// The HTTP edge — a route's `Authorize<A, E>` shaper.
    #[cfg(feature = "http")]
    pub const HTTP: &str = "http";
    /// The GraphQL edge — a resolver operation or a federation root field.
    #[cfg(feature = "graphql")]
    pub const GRAPHQL: &str = "graphql";
    /// The WebSocket edge — one message on an established connection.
    #[cfg(feature = "ws")]
    pub const WS: &str = "ws";
    /// The MCP edge — one tool call or prompt fetch.
    #[cfg(feature = "mcp")]
    pub const MCP: &str = "mcp";
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
///
/// A record rather than seven arguments because each site knows a different
/// three of them: a class gate names the action and subject the posture
/// declared, a guard entry names neither (it decides before any posture is
/// read), and a response mask names the entity plus the keys a field grant
/// stripped. Struct-update syntax off [`Refusal::on`] lets a site state its own
/// and stay silent on the rest — and a field added here reaches every site at
/// once, which is the whole reason the emitter is shared.
#[derive(Clone, Copy)]
pub struct Refusal<'a> {
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
    /// What the developer would change. Prose, and deliberately **not** the
    /// `reason` value: an incident groups by the second and reads the first,
    /// and a sentence in that slot is a value space of one per edge.
    pub remedy: Option<&'static str>,
}

impl<'a> Refusal<'a> {
    /// A refusal on `transport`, with nothing else stated yet.
    pub fn on(transport: &'static str) -> Self {
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
    pub fn of<A: ActionMarker, S: Subject>(transport: &'static str) -> Self {
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
///
/// Beside [`gate`] rather than in each transport, because unlike the `Opaque`
/// seams — whose messages are deliberately different per edge — the target and
/// the event name here are the *same literal* on every edge, and only fields
/// vary. A field added for one transport and missed on the others is exactly
/// what having a copy per transport buys, and so is a second *name* for one
/// decision: a mask's field refusal filed under its own event name is a denial
/// an incident query by `reason` never returns.
pub fn warn_denied(refusal: Refusal<'_>) {
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
        // `Option<Action>` is not a `tracing` value; the action's own `Debug`
        // rendering is what every site printed before, and `field::debug`
        // keeps it while letting the field be absent where no posture named
        // one.
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
