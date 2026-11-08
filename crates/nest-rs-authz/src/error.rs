//! Typed errors for the authorization layers.

use std::fmt;

use crate::action::Action;
use crate::contained::Uncontained;

/// A rule the builder refused, so [`AbilityBuilder::build`] fails rather than an
/// ability scoping rows other than the rule says: a relational predicate
/// [`PredicateBuilder::related`] rejected (a composite key, or a relation not
/// pointing at the declared related entity), or a condition whose text would not
/// stay where the scope writes it (a relation's `on_condition`, a value's enum type
/// name).
///
/// Unchecked, a malformed `cannot(...)` fails open: its `1 = 0` condition under
/// `grant AND NOT(deny)` matches every row, and a text leaving its place in the scope
/// reaches past the rules beside it.
///
/// [`PredicateBuilder::related`]: crate::predicate::PredicateBuilder::related
/// [`AbilityBuilder::build`]: crate::AbilityBuilder::build
#[derive(Debug, thiserror::Error)]
#[error("malformed authorization rule: the {kind} for `{action:?}` on `{subject}` {defect}")]
#[non_exhaustive]
pub struct MalformedRuleError {
    /// The action the faulty rule was declared for.
    pub action: Action,
    /// Type name of the subject entity the faulty rule scopes.
    pub subject: &'static str,
    /// `"grant"` (a `can`) or `"denial"` (a `cannot`) — a denial is the
    /// fail-open case, a grant merely the silent one.
    pub kind: &'static str,
    defect: Defect,
}

impl MalformedRuleError {
    /// The rule `kind` for `action` on `subject`, refused for `defect`.
    pub(crate) fn new(
        action: Action,
        subject: &'static str,
        kind: &'static str,
        defect: Defect,
    ) -> Self {
        Self {
            action,
            subject,
            kind,
            defect,
        }
    }
}

/// What made a rule malformed, which its refusal says.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Defect {
    /// `related(...)` rejected its relation.
    Relation,
    /// Its lowered condition's text would not stay where the scope writes it.
    Uncontained(Uncontained),
}

impl fmt::Display for Defect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relation => f.write_str(
                "uses an invalid relation predicate — the relation is composite-keyed or does \
                 not point at the related entity. Fix the `related(...)` call in your \
                 `AbilityFactory`.",
            ),
            Self::Uncontained(why) => write!(
                f,
                "writes text into its condition that would not stay where the scope puts it \
                 ({}) — a relation's `on_condition` or a value's enum type name. Build them \
                 with sea-query's typed calls, every parenthesis, string and identifier they \
                 open closed inside them, no comment, and no `\"` in a type name.",
                why.as_str(),
            ),
        }
    }
}

/// A write setting columns no grant of its action permits, refused before any
/// statement: what [`Ability::condition_for_columns`](crate::Ability::condition_for_columns)
/// answers for a column `.fields` leaves out of every grant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no grant permits writing {}", .columns.join(", "))]
#[non_exhaustive]
pub struct ColumnsRefused {
    /// Each column refused, as `Column::as_str()` names it, in the order asked.
    pub columns: Vec<&'static str>,
}

/// Why `masked_reply` could not produce a masked value. Callers must treat
/// either case as fail-closed: send an error frame, never the unmasked body.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MaskReplyError {
    /// No ambient [`Ability`](crate::Ability) — no ability guard ran at the
    /// request, so masking cannot run.
    #[error("no ambient ability — does an ability guard run at the request?")]
    NoAmbientAbility,
    /// The wire value could not be reconciled with the entity model — said by
    /// where and what kind, never by the value.
    #[error("wire value could not be reconciled with the entity model")]
    Irreconcilable(#[source] nest_rs_core::DecodeError),
}
