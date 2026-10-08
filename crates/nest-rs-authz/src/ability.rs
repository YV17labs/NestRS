//! The compiled rule set for one actor, and the four reads the three
//! authorization layers (gate, query filter, response mask) perform against it.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

use sea_orm::EntityTrait;
use sea_orm::sea_query::{Condition, Expr};

use crate::action::Action;
use crate::predicate::Predicate;

/// The one fail-closed masking warn, for every edge and the ambient
/// [`Ability::mask`]; here, always compiled, so a feature-less build reaches it.
pub(crate) fn warn_mask_failure(
    entity: &'static str,
    action: Action,
    reason: &'static str,
    detail: &'static str,
    // The edge's own context, on this one event; `tracing` drops a `None`.
    transport: Option<&'static str>,
    event: Option<&str>,
    // Rendered by `error_message`, which never quotes the value serde refused.
    err: Option<&(dyn std::error::Error + 'static)>,
) {
    // Two arms: `tracing` fixes an event's fields at the macro.
    match err {
        Some(err) => tracing::warn!(
            target: crate::TARGET,
            entity,
            action = ?action,
            reason,
            detail,
            transport,
            event,
            error = %nest_rs_core::error_message(err),
            "response masking failed",
        ),
        None => tracing::warn!(
            target: crate::TARGET,
            entity,
            action = ?action,
            reason,
            detail,
            transport,
            event,
            "response masking failed",
        ),
    }
}

/// Why a mask fell closed, as a value an incident query groups on.
pub(crate) mod mask_reason {
    /// Nothing installed an ability, so nothing decided what may be shown.
    ///
    /// Declared here and aliased by [`gate::reason`](crate::gate::reason):
    /// `gate` is feature-gated, this module is not.
    pub(crate) const NO_AMBIENT_ABILITY: &str = "no_ambient_ability";
    /// The value could not be turned into, or read back from, JSON.
    pub(crate) const NOT_SERIALIZABLE: &str = "not_serializable";
    /// The wire value and the entity model could not be reconciled, so the
    /// mask had no model to apply.
    #[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
    pub(crate) const IRRECONCILABLE: &str = "irreconcilable_wire_value";
    /// The response carried no content type, so it could not be classified as
    /// maskable at all. HTTP's alone — it is the one edge with a content type
    /// to be missing.
    #[cfg(feature = "http")]
    pub(crate) const UNCLASSIFIED_BODY: &str = "unclassified_body";
    // `field_not_granted` is a gate verdict, owned by `gate::reason::FIELD_NOT_GRANTED`.
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
    /// Read it only after [`can_class`](Self::can_class) said no: a withheld rule
    /// and a granted one can coexist. Empty means the refusal was not about scope.
    pub fn missing_scopes(&self, action: Action, subject: TypeId) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
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
    /// The enforcement layers ignore it; a transport admitting anonymous callers
    /// reads it so a `define_visitor` grant cannot satisfy an `#[authorize]`
    /// operation. Read it from a guard or a posture attribute's gate only.
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
    /// [`mask_many`](Self::mask_many): one pass yields both answers.
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
    /// permit for `action`.
    pub fn mask<E>(&self, action: Action, model: &E::Model) -> serde_json::Value
    where
        E: EntityTrait,
        E::Model: serde::Serialize,
    {
        self.mask_with::<E>(action, model, self.permitted_fields::<E>(action, model))
    }

    /// [`mask`](Self::mask) with the field verdict already known.
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
            // Fail closed: an empty body, never the unmasked model.
            Err(err) => {
                warn_mask_failure(
                    std::any::type_name::<E>(),
                    action,
                    mask_reason::NOT_SERIALIZABLE,
                    "model did not serialize",
                    None,
                    None,
                    Some(&err),
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

/// Recover a rule's typed predicate. A mismatch cannot happen (rules are keyed
/// by `TypeId::of::<E>()`), and fails closed at the call sites, never panics.
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
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod gadget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
        #[sea_orm(table_name = "gadgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// A rule keyed under one subject carrying another's — `AbilityBuilder` cannot build it.
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

    #[test]
    fn a_mask_failure_names_what_kind_of_value_it_found_never_the_value() {
        #[derive(Debug, serde::Deserialize)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts the error, never a value"
        )]
        struct Wire {
            age: u64,
        }
        let logs = nest_rs_testing::LogCapture::install();
        let refused =
            serde_json::from_value::<Wire>(serde_json::json!({ "age": "ada@example.com" }))
                .expect_err("a string is no age");
        warn_mask_failure(
            "Widget",
            Action::Read,
            mask_reason::IRRECONCILABLE,
            "wire value could not be reconciled with the entity model",
            None,
            None,
            Some(&refused),
        );
        let event = logs.expect_one(crate::TARGET, "response masking failed");
        assert_eq!(
            event.field("error").as_deref(),
            Some("invalid type: a string, expected u64")
        );
    }
}
