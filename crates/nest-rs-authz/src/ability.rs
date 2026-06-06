//! The compiled rule set for one actor, and the four reads the three
//! authorization layers (gate, query filter, response mask) perform against it.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

use sea_orm::EntityTrait;
use sea_orm::sea_query::{Condition, Expr};

use crate::action::Action;
use crate::predicate::Predicate;

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
}

impl Ability {
    pub(crate) fn add_rule(&mut self, action: Action, subject: TypeId, rule: Rule) {
        self.rules.entry((action, subject)).or_default().push(rule);
    }

    /// Rules relevant to `action` on `subject`: those keyed under the action
    /// itself plus those under [`Action::Manage`] (the action wildcard).
    fn rules_for(&self, action: Action, subject: TypeId) -> impl Iterator<Item = &Rule> {
        let specific = self.rules.get(&(action, subject)).into_iter().flatten();
        let wildcard = if action == Action::Manage {
            None
        } else {
            self.rules.get(&(Action::Manage, subject))
        };
        specific.chain(wildcard.into_iter().flatten())
    }

    /// Layer ① — the coarse, class-level gate the access guard/extractor uses:
    /// is there *any* grant for this action on this subject? Optimistic like
    /// CASL — instance conditions are enforced by layers ② and ③, not here.
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
        let mut allowed = false;
        for rule in self.rules_for(action, TypeId::of::<E>()) {
            if predicate_of::<E>(rule).matches(model) {
                if rule.inverted {
                    return false;
                }
                allowed = true;
            }
        }
        allowed
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
        let mut json = serde_json::to_value(model).unwrap_or(serde_json::Value::Null);
        if let FieldSet::Only(allowed) = self.permitted_fields::<E>(action, model)
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
            .filter(|model| self.can::<E>(action, model))
            .map(|model| self.mask::<E>(action, model))
            .collect()
    }

    /// Layer ③ — the union of permitted fields across the grants that match this
    /// model. An unrestricted matching grant permits every field.
    pub fn permitted_fields<E: EntityTrait>(&self, action: Action, model: &E::Model) -> FieldSet {
        let mut acc: HashSet<&'static str> = HashSet::new();
        for rule in self
            .rules_for(action, TypeId::of::<E>())
            .filter(|rule| !rule.inverted)
        {
            if !predicate_of::<E>(rule).matches(model) {
                continue;
            }
            match &rule.fields {
                FieldSet::All => return FieldSet::All,
                FieldSet::Only(cols) => acc.extend(cols.iter().copied()),
            }
        }
        FieldSet::Only(acc)
    }
}

/// Recover a rule's typed predicate. The downcast cannot fail: the rule was
/// stored under `TypeId::of::<E>()`, so its predicate is a `Predicate<E>`.
fn predicate_of<E: EntityTrait>(rule: &Rule) -> &Predicate<E> {
    rule.predicate
        .downcast_ref::<Predicate<E>>()
        .expect("rule predicate type matches the subject it is keyed under")
}
