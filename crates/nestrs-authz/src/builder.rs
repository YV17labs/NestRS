//! The fluent builder an [`AbilityFactory`](crate::AbilityFactory) writes rules
//! against: `ab.can(Action::Read, users::Entity).when(|p| …).fields([…])`.
//!
//! A [`RuleSpec`] finalizes itself on drop, so a rule is committed simply by
//! ending the statement — there is no terminal call to forget.

use std::any::TypeId;

use sea_orm::{EntityTrait, IdenStatic};

use crate::ability::{Ability, FieldSet, Rule};
use crate::action::Action;
use crate::predicate::{Predicate, PredicateBuilder};

/// Collects the rules an [`AbilityFactory`](crate::AbilityFactory) declares for
/// one actor, then yields the compiled [`Ability`].
#[derive(Default)]
pub struct AbilityBuilder {
    ability: Ability,
}

impl AbilityBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a grant: `can(action, Subject)` allows `action` on `Subject`,
    /// optionally narrowed by [`when`](RuleSpec::when) / [`fields`](RuleSpec::fields).
    pub fn can<E>(&mut self, action: Action, _subject: E) -> RuleSpec<'_, E>
    where
        E: EntityTrait,
        E::Column: Send + Sync + 'static,
    {
        RuleSpec::new(self, action, false)
    }

    /// Begin a denial — the same shape as [`can`](AbilityBuilder::can) but
    /// subtracts from what grants allow (a matching denial overrides).
    pub fn cannot<E>(&mut self, action: Action, _subject: E) -> RuleSpec<'_, E>
    where
        E: EntityTrait,
        E::Column: Send + Sync + 'static,
    {
        RuleSpec::new(self, action, true)
    }

    /// The compiled ability.
    pub fn build(self) -> Ability {
        self.ability
    }
}

/// One in-progress rule. Commits to the [`AbilityBuilder`] on drop, so each
/// rule must be its own complete statement — binding it to a variable defers
/// the commit, and the builder cannot be reused while a spec is still alive.
pub struct RuleSpec<'a, E>
where
    E: EntityTrait,
    E::Column: Send + Sync + 'static,
{
    builder: &'a mut AbilityBuilder,
    action: Action,
    inverted: bool,
    predicate: Predicate<E>,
    fields: FieldSet,
}

impl<'a, E> RuleSpec<'a, E>
where
    E: EntityTrait,
    E::Column: Send + Sync + 'static,
{
    fn new(builder: &'a mut AbilityBuilder, action: Action, inverted: bool) -> Self {
        Self {
            builder,
            action,
            inverted,
            predicate: Predicate::Always,
            fields: FieldSet::All,
        }
    }

    /// Narrow the rule with a row condition. The closure builds it against the
    /// subject's columns: `.when(|p| p.eq(users::Column::OrgId, actor.org_id))`.
    pub fn when(mut self, build: impl FnOnce(PredicateBuilder<E>) -> Predicate<E>) -> Self {
        self.predicate = build(PredicateBuilder::new());
        self
    }

    /// Restrict the rule to these columns — the response masker keeps only these
    /// fields. Without this, every field is permitted.
    pub fn fields(mut self, columns: impl IntoIterator<Item = E::Column>) -> Self {
        self.fields = FieldSet::Only(columns.into_iter().map(|c| c.as_str()).collect());
        self
    }
}

impl<'a, E> Drop for RuleSpec<'a, E>
where
    E: EntityTrait,
    E::Column: Send + Sync + 'static,
{
    fn drop(&mut self) {
        let condition = self.predicate.to_condition();
        let predicate = std::mem::take(&mut self.predicate);
        let fields = std::mem::take(&mut self.fields);
        self.builder.ability.add_rule(
            self.action,
            TypeId::of::<E>(),
            Rule {
                inverted: self.inverted,
                condition,
                predicate: Box::new(predicate),
                fields,
            },
        );
    }
}
