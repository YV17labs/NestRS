//! Fluent builder for `AbilityFactory`:
//! `ab.can(Action::Read, users::Entity).when(|p| …).fields([…])`.
//!
//! A [`RuleSpec`] commits on drop — the rule is finalized by ending the
//! statement, with no terminal call to forget.

use std::any::TypeId;

use sea_orm::{EntityTrait, IdenStatic};

use crate::ability::{Ability, FieldSet, Rule};
use crate::action::Action;
use crate::predicate::{Predicate, PredicateBuilder};

#[derive(Default)]
pub struct AbilityBuilder {
    ability: Ability,
}

impl AbilityBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a grant. Narrow with [`when`](RuleSpec::when) / [`fields`](RuleSpec::fields).
    pub fn can<E>(&mut self, action: Action, _subject: E) -> RuleSpec<'_, E>
    where
        E: EntityTrait,
        E::Column: Send + Sync + 'static,
    {
        RuleSpec::new(self, action, false)
    }

    /// Begin a denial — a matching denial overrides a matching grant.
    pub fn cannot<E>(&mut self, action: Action, _subject: E) -> RuleSpec<'_, E>
    where
        E: EntityTrait,
        E::Column: Send + Sync + 'static,
    {
        RuleSpec::new(self, action, true)
    }

    pub fn build(self) -> Ability {
        self.ability
    }
}

/// One in-progress rule. Commits on drop — binding to a variable defers the
/// commit, and the builder cannot be reused while a spec is still alive.
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

    /// `.when(|p| p.eq(users::Column::OrgId, actor.org_id))`.
    pub fn when(mut self, build: impl FnOnce(PredicateBuilder<E>) -> Predicate<E>) -> Self {
        self.predicate = build(PredicateBuilder::new());
        self
    }

    /// Restrict the rule to these columns — the response masker keeps only
    /// these fields. Without this, every field is permitted.
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
