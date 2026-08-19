//! The compiled rule set for one actor, and the four reads the three
//! authorization layers (gate, query filter, response mask) perform against it.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

use sea_orm::EntityTrait;
use sea_orm::sea_query::{Condition, Expr};

use crate::action::Action;
use crate::predicate::Predicate;

/// One place for the fail-closed masking warn, so every branch — HTTP, GraphQL,
/// and the ambient [`Ability::mask`] — emits the identical queryable event
/// (`target: "nest_rs::authz"`, same keys) instead of hand-copying it. A
/// fail-closed branch that forgets to log is then the visible omission. Lives
/// here (always compiled) rather than in `wire_mask` (gated behind the
/// `http`/`graphql` transports) so the ambient `Ability::mask` can reach it in
/// a feature-less build.
pub(crate) fn warn_mask_failure(
    entity: &'static str,
    action: Action,
    reason: &'static str,
    err: &dyn std::fmt::Display,
) {
    tracing::warn!(
        target: crate::TARGET,
        entity,
        action = ?action,
        reason,
        error = %err,
        "response masking failed",
    );
}

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

/// Which fields of a subject may be read back in the response.
#[derive(Default)]
pub enum FieldSet {
    /// No restriction — every field is permitted.
    #[default]
    All,
    /// Only these columns (named as they serialize) are permitted.
    Only(HashSet<&'static str>),
}

/// One grant or denial. The condition is precomputed at build time (the actor's
/// values are known then); the typed [`Predicate`] is kept type-erased for the
/// in-memory check, downcast at the call site where the subject type is known.
pub(crate) struct Rule {
    pub(crate) inverted: bool,
    pub(crate) condition: Condition,
    pub(crate) predicate: Box<dyn Any + Send + Sync>,
    pub(crate) fields: FieldSet,
}

/// The authorization rules compiled for a single actor. Built by an
/// [`AbilityFactory`](crate::AbilityFactory) and consumed by the access guard
/// ([`can_class`](Ability::can_class)), the query pre-filter
/// ([`condition_for`](Ability::condition_for)), and the response check/mask
/// ([`can`](Ability::can) / [`permitted_fields`](Ability::permitted_fields)).
#[derive(Default)]
pub struct Ability {
    rules: HashMap<(Action, TypeId), Vec<Rule>>,
    /// Scopes that would have unlocked a rule this actor's credential could not
    /// reach — recorded when [`RuleSpec::requires_scope`] withholds one, so a
    /// refusal can name what to ask for instead of being an opaque `403`.
    ///
    /// [`RuleSpec::requires_scope`]: crate::RuleSpec::requires_scope
    withheld: HashMap<(Action, TypeId), Vec<String>>,
    visitor: bool,
}

impl Ability {
    pub(crate) fn add_rule(&mut self, action: Action, subject: TypeId, rule: Rule) {
        self.rules.entry((action, subject)).or_default().push(rule);
    }

    /// Record that a rule was withheld for lack of `scopes`. The rule itself is
    /// never added — a withheld grant must not widen anything.
    pub(crate) fn withhold(&mut self, action: Action, subject: TypeId, scopes: Vec<String>) {
        let entry = self.withheld.entry((action, subject)).or_default();
        for scope in scopes {
            if !entry.contains(&scope) {
                entry.push(scope);
            }
        }
    }

    /// The scopes that would have granted `action` on `subject`, had this
    /// actor's credential carried them.
    ///
    /// **Read this only after [`can_class`](Self::can_class) already said no.**
    /// A withheld rule and a granted one can coexist — a narrow token may still
    /// reach the subject by another rule — and in that case the operation is
    /// allowed and there is nothing to ask for. Reading it as "the caller is
    /// missing these scopes" without checking the gate first would report a
    /// denial that never happened.
    ///
    /// Empty means the refusal was not about scope: the caller may not perform
    /// this operation at all, and no wider token changes that.
    pub fn missing_scopes(&self, action: Action, subject: TypeId) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        // Widened over the same keys as `rules_for`: a `Manage` rule is the
        // action wildcard, so a scope withheld there is equally the answer to
        // "why can't I read this?". Encoding that widening twice is how the
        // refusal comes to name the wrong scopes.
        for scope in keys_for(action, subject)
            .filter_map(|key| self.withheld.get(&key))
            .flatten()
        {
            if !out.contains(scope) {
                out.push(scope.clone());
            }
        }
        out
    }

    pub(crate) fn mark_visitor(&mut self) {
        self.visitor = true;
    }

    /// Whether these rules came from
    /// [`AbilityFactory::define_visitor`](crate::AbilityFactory::define_visitor)
    /// — i.e. the caller is **anonymous**.
    ///
    /// A grant is a grant on either branch, so the three enforcement layers
    /// ignore this. It answers the *other* question, the one a transport whose
    /// edge admits anonymous callers has to ask before running a gate: is there
    /// a principal at all? On HTTP that is the route's own posture (a
    /// non-`#[public]` route never reaches the visitor branch); on GraphQL,
    /// where the single `/graphql` endpoint is `#[public]` and posture is
    /// declared per operation, `graphql::authorize`
    /// reads this so a `define_visitor` grant cannot satisfy an
    /// `#[authorize(...)]` operation.
    ///
    /// Read it from a **guard** or from the gate a posture attribute emits, the
    /// same rule [`can_class`](Self::can_class) follows — a check buried in a
    /// service or a parameter type is an authorization decision outside the
    /// three greppable sites.
    pub fn is_visitor(&self) -> bool {
        self.visitor
    }

    /// Rules relevant to `action` on `subject`: those keyed under the action
    /// itself plus those under [`Action::Manage`] (the action wildcard).
    fn rules_for(&self, action: Action, subject: TypeId) -> impl Iterator<Item = &Rule> {
        keys_for(action, subject)
            .filter_map(|key| self.rules.get(&key))
            .flatten()
    }

    /// Layer ① — the coarse, class-level gate the access guard/extractor uses:
    /// is there *any* grant for this action on this subject? Optimistic —
    /// instance conditions are enforced by layers ② and ③, not here.
    pub fn can_class(&self, action: Action, subject: TypeId) -> bool {
        self.rules_for(action, subject).any(|rule| !rule.inverted)
    }

    /// Layer ② — the query pre-filter: `(OR of grant conditions) AND NOT (OR of
    /// denial conditions)`. With no grant the result matches nothing (`1 = 0`).
    pub fn condition_for<E: EntityTrait>(&self, action: Action) -> Condition {
        let mut grant = Condition::any();
        let mut deny = Condition::any();
        for rule in self.rules_for(action, TypeId::of::<E>()) {
            if rule.inverted {
                deny = deny.add(rule.condition.clone());
            } else {
                grant = grant.add(rule.condition.clone());
            }
        }
        if grant.is_empty() {
            return Condition::all().add(Expr::cust("1 = 0"));
        }
        let mut out = Condition::all().add(grant);
        if !deny.is_empty() {
            out = out.add(deny.not());
        }
        out
    }

    /// Layer ③ — instance check: at least one grant matches this model and no
    /// denial does (a denial overrides).
    pub fn can<E: EntityTrait>(&self, action: Action, model: &E::Model) -> bool {
        self.evaluate::<E>(action, model).allowed
    }

    /// The single rule scan behind [`can`](Self::can),
    /// [`permitted_fields`](Self::permitted_fields) and
    /// [`mask_many`](Self::mask_many) — both answers come from the same
    /// predicates, so computing them together is one pass instead of two per
    /// row (the masked-list path used to evaluate every predicate twice).
    ///
    /// `pub(crate)` for the same reason: a caller needing *both* answers about
    /// one row asks once here rather than calling `can` and then
    /// `permitted_fields`.
    pub(crate) fn evaluate<E: EntityTrait>(&self, action: Action, model: &E::Model) -> Verdict {
        let mut granted = false;
        let mut denied = false;
        let mut unrestricted = false;
        let mut fields: HashSet<&'static str> = HashSet::new();
        for rule in self.rules_for(action, TypeId::of::<E>()) {
            let Some(predicate) = predicate_of::<E>(rule) else {
                // Unreachable type mismatch (see `predicate_of`) — fail
                // closed: a broken denial denies, a broken grant never widens.
                denied |= rule.inverted;
                continue;
            };
            if !predicate.matches(model) {
                continue;
            }
            if rule.inverted {
                // A denial overrides every grant, whatever their order.
                denied = true;
                continue;
            }
            granted = true;
            match &rule.fields {
                FieldSet::All => unrestricted = true,
                FieldSet::Only(cols) => {
                    if !unrestricted {
                        fields.extend(cols.iter().copied());
                    }
                }
            }
        }
        Verdict {
            allowed: granted && !denied,
            // Field grants are read from the matching grants alone — denials
            // decide visibility, never which columns a visible row exposes.
            fields: if unrestricted {
                FieldSet::All
            } else {
                FieldSet::Only(fields)
            },
        }
    }

    /// Layer ③ — serialize a model and strip the fields this ability does not
    /// permit for `action`. Returns the masked JSON object. Combined with the
    /// query pre-filter this is defence in depth: the filter keeps the wrong
    /// rows out of the result, the mask keeps the wrong fields out of the body.
    pub fn mask<E>(&self, action: Action, model: &E::Model) -> serde_json::Value
    where
        E: EntityTrait,
        E::Model: serde::Serialize,
    {
        self.mask_with::<E>(action, model, self.permitted_fields::<E>(action, model))
    }

    /// [`mask`](Self::mask) with the field verdict already known — the seam
    /// [`mask_many`](Self::mask_many) and the per-item subscription path use so
    /// a row's rules are evaluated once for "may I see it?" and "which
    /// columns?" together.
    pub(crate) fn mask_with<E>(
        &self,
        action: Action,
        model: &E::Model,
        fields: FieldSet,
    ) -> serde_json::Value
    where
        E: EntityTrait,
        E::Model: serde::Serialize,
    {
        let mut json = match serde_json::to_value(model) {
            Ok(json) => json,
            // Practically unreachable for a SeaORM model, but fail *safe* (an
            // empty body, never the unmasked model) and — unlike the previous
            // silent `unwrap_or` — leave a queryable trace, matching the
            // wire-mask paths. A hard fail-closed (500 / GraphQL error) would
            // need `mask`'s signature to become `Result`, rippling through
            // `mask_many` and both transports; logged `Null` is the surgical fix.
            Err(err) => {
                warn_mask_failure(
                    std::any::type_name::<E>(),
                    action,
                    "model did not serialize",
                    &err,
                );
                return serde_json::Value::Null;
            }
        };
        if let FieldSet::Only(allowed) = fields
            && let serde_json::Value::Object(map) = &mut json
        {
            map.retain(|key, _| allowed.contains(key.as_str()));
        }
        json
    }

    /// Layer ③ over a collection: drop the instances the actor may not see
    /// ([`can`](Ability::can)) and mask the fields of those it may
    /// ([`mask`](Ability::mask)).
    pub fn mask_many<'m, E>(
        &self,
        action: Action,
        models: impl IntoIterator<Item = &'m E::Model>,
    ) -> Vec<serde_json::Value>
    where
        E: EntityTrait,
        E::Model: serde::Serialize + 'm,
    {
        models
            .into_iter()
            .filter_map(|model| {
                let verdict = self.evaluate::<E>(action, model);
                verdict
                    .allowed
                    .then(|| self.mask_with::<E>(action, model, verdict.fields))
            })
            .collect()
    }

    /// Layer ③ — the union of permitted fields across the grants that match this
    /// model. An unrestricted matching grant permits every field.
    pub fn permitted_fields<E: EntityTrait>(&self, action: Action, model: &E::Model) -> FieldSet {
        self.evaluate::<E>(action, model).fields
    }
}

/// The rule-map keys an operation reads: the action itself, plus
/// [`Action::Manage`] (the action wildcard) unless that *is* the action.
///
/// The single encoding of the wildcard's semantics — both the grant side
/// ([`Ability::rules_for`]) and the refusal side ([`Ability::missing_scopes`])
/// iterate it, so a change to the action lattice cannot widen one and not the
/// other.
fn keys_for(action: Action, subject: TypeId) -> impl Iterator<Item = (Action, TypeId)> {
    let wildcard = (action != Action::Manage).then_some((Action::Manage, subject));
    std::iter::once((action, subject)).chain(wildcard)
}

/// What one rule scan concluded about a model: whether it is visible, and which
/// of its columns the matching grants expose.
pub(crate) struct Verdict {
    pub(crate) allowed: bool,
    pub(crate) fields: FieldSet,
}

/// Recover a rule's typed predicate. The downcast cannot fail in practice —
/// the rule was stored under `TypeId::of::<E>()`, so its predicate is a
/// `Predicate<E>` — but this is a per-request authz path, so a mismatch fails
/// **closed** at the call sites (deny / no grant) instead of panicking,
/// mirroring `Predicate::to_condition`'s defense-in-depth posture.
fn predicate_of<E: EntityTrait>(rule: &Rule) -> Option<&Predicate<E>> {
    let predicate = rule.predicate.downcast_ref::<Predicate<E>>();
    if predicate.is_none() {
        tracing::error!(
            target: crate::TARGET,
            reason = "predicate_type_mismatch",
            "ability rule predicate does not match its keyed subject — failing closed",
        );
    }
    predicate
}

#[cfg(test)]
mod tests {
    use super::*;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
        #[sea_orm(table_name = "widgets")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod gadget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
        #[sea_orm(table_name = "gadgets")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// A rule keyed under one subject carrying another subject's predicate.
    ///
    /// `AbilityBuilder` cannot produce this — it stores the predicate under the
    /// same `TypeId` it built it for — which is exactly why the branch needs a
    /// test: nothing else will ever exercise it, and what it decides is whether
    /// a mismatch **denies** or reads as an unrestricted grant. The second
    /// would turn a framework bug into a silent authorization bypass, so the
    /// answer is `None` (no grant) and a line loud enough to find.
    fn mismatched_rule() -> Rule {
        Rule {
            inverted: false,
            condition: Condition::all(),
            predicate: Box::new(Predicate::<gadget::Entity>::Always),
            fields: FieldSet::All,
        }
    }

    #[test]
    fn a_predicate_of_another_subject_yields_no_grant_and_is_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        let rule = mismatched_rule();

        assert!(
            predicate_of::<widget::Entity>(&rule).is_none(),
            "a predicate that is not this subject's grants nothing — reading it \
             as unrestricted is the one answer that opens rows",
        );

        let event = logs.expect_one(
            "nest_rs::authz",
            "ability rule predicate does not match its keyed subject — failing closed",
        );
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field("reason").as_deref(),
            Some("predicate_type_mismatch"),
            "{:?}",
            event.fields,
        );
    }

    #[test]
    fn a_predicate_of_the_keyed_subject_is_recovered_in_silence() {
        // The other direction: every rule in every ability goes through this,
        // so a check reading the wrong thing would deny the whole app.
        let logs = nest_rs_testing::LogCapture::install();
        let rule = Rule {
            inverted: false,
            condition: Condition::all(),
            predicate: Box::new(Predicate::<widget::Entity>::Always),
            fields: FieldSet::All,
        };

        assert!(predicate_of::<widget::Entity>(&rule).is_some());
        logs.expect_none(
            "nest_rs::authz",
            "ability rule predicate does not match its keyed subject — failing closed",
        );
    }
}
