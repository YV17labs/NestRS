//! Typed errors for the authorization layers.

use crate::action::Action;

/// A rule whose relational predicate was malformed — [`PredicateBuilder::related`]
/// rejected it (a composite key, or a relation not pointing at the declared
/// related entity) and produced the [`Predicate::Deny`] sentinel. Raised by
/// [`AbilityBuilder::build`] so the misconfiguration fails **loudly** at ability
/// construction rather than silently.
///
/// Left unchecked this is a security defect on the denial side: a malformed
/// `cannot(...)` lowers its condition to `1 = 0`, and the query pre-filter
/// combines a denial as `grant AND NOT(deny)` — so `NOT(1 = 0)` is *true* and
/// the restriction evaporates (fail-*open*). On the grant side the same
/// sentinel is fail-closed (deny-all) but still hides a developer error, so
/// both are surfaced.
///
/// [`PredicateBuilder::related`]: crate::predicate::PredicateBuilder::related
/// [`Predicate::Deny`]: crate::predicate::Predicate::Deny
/// [`AbilityBuilder::build`]: crate::AbilityBuilder::build
#[derive(Debug, thiserror::Error)]
#[error(
    "malformed authorization rule: the {kind} for `{action:?}` on `{subject}` uses an invalid \
     relation predicate — the relation is composite-keyed or does not point at the related \
     entity. Fix the `related(...)` call in your `AbilityFactory`."
)]
pub struct MalformedRuleError {
    /// The action the faulty rule was declared for.
    pub action: Action,
    /// Type name of the subject entity the faulty rule scopes.
    pub subject: &'static str,
    /// `"grant"` (a `can`) or `"denial"` (a `cannot`) — a denial is the
    /// fail-open case, a grant merely the silent one.
    pub kind: &'static str,
}

/// Why [`masked_reply`](crate::masked_reply) could not produce a masked value. Callers must treat
/// either case as fail-closed: send an error frame, never the unmasked body.
#[derive(Debug, thiserror::Error)]
pub enum MaskReplyError {
    /// No ambient [`Ability`](crate::Ability) is installed — the auth bridge for this
    /// transport is missing, so masking cannot run.
    #[error("no ambient ability — is the transport's authz bridge installed?")]
    NoAmbientAbility,
    /// The wire value could not be reconciled with the entity model — said by
    /// where and what kind, never by the value.
    #[error("wire value could not be reconciled with the entity model")]
    Irreconcilable(#[source] nest_rs_core::DecodeError),
}
