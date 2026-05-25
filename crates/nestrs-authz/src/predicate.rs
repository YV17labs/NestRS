//! The one condition representation that feeds both authorization layers that
//! need it: [`Predicate::to_condition`] lowers it to a SQL `WHERE` fragment for
//! the query pre-filter, and [`Predicate::matches`] evaluates it in memory
//! against a loaded model for the response check. Sharing a single AST is what
//! keeps the rows a query returns and the rows the response check accepts from
//! drifting apart.

use std::marker::PhantomData;

use sea_orm::sea_query::Condition;
use sea_orm::{ColumnTrait, EntityTrait, ModelTrait, Value};

/// A condition over entity `E`, interpreted as SQL or in memory.
#[derive(Default)]
pub enum Predicate<E: EntityTrait> {
    /// Matches everything — an unconditional grant.
    #[default]
    Always,
    /// `column = value`.
    Eq(E::Column, Value),
    /// `column IN (values)`.
    In(E::Column, Vec<Value>),
    And(Vec<Predicate<E>>),
    Or(Vec<Predicate<E>>),
    Not(Box<Predicate<E>>),
}

impl<E: EntityTrait> Predicate<E> {
    /// Lower to a [`sea_orm::Condition`] for the query pre-filter. An empty
    /// `Condition::all()` is the SQL identity (`TRUE`), so [`Predicate::Always`]
    /// imposes no constraint.
    pub fn to_condition(&self) -> Condition {
        match self {
            Predicate::Always => Condition::all(),
            Predicate::Eq(col, value) => Condition::all().add(col.eq(value.clone())),
            Predicate::In(col, values) => Condition::all().add(col.is_in(values.clone())),
            Predicate::And(parts) => parts
                .iter()
                .fold(Condition::all(), |acc, p| acc.add(p.to_condition())),
            Predicate::Or(parts) => parts
                .iter()
                .fold(Condition::any(), |acc, p| acc.add(p.to_condition())),
            Predicate::Not(inner) => inner.to_condition().not(),
        }
    }

    /// Evaluate in memory against a loaded model — the response-side check.
    /// Reads each column with [`ModelTrait::get`], which returns the same
    /// [`Value`] the SQL side compares against.
    pub fn matches(&self, model: &E::Model) -> bool {
        match self {
            Predicate::Always => true,
            Predicate::Eq(col, value) => &model.get(*col) == value,
            Predicate::In(col, values) => {
                let actual = model.get(*col);
                values.iter().any(|v| v == &actual)
            }
            Predicate::And(parts) => parts.iter().all(|p| p.matches(model)),
            Predicate::Or(parts) => parts.iter().any(|p| p.matches(model)),
            Predicate::Not(inner) => !inner.matches(model),
        }
    }
}

/// Fluent constructor handed to the `when(|p| …)` closure of a rule, so a rule's
/// condition reads `p.eq(Column::OrgId, actor.org_id)` without naming
/// [`Predicate`] or [`Value`] directly.
pub struct PredicateBuilder<E: EntityTrait>(PhantomData<fn() -> E>);

impl<E: EntityTrait> PredicateBuilder<E> {
    pub(crate) fn new() -> Self {
        PredicateBuilder(PhantomData)
    }

    pub fn eq(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Eq(column, value.into())
    }

    pub fn is_in<V: Into<Value>>(
        &self,
        column: E::Column,
        values: impl IntoIterator<Item = V>,
    ) -> Predicate<E> {
        Predicate::In(column, values.into_iter().map(Into::into).collect())
    }

    pub fn all(&self, parts: impl IntoIterator<Item = Predicate<E>>) -> Predicate<E> {
        Predicate::And(parts.into_iter().collect())
    }

    pub fn any(&self, parts: impl IntoIterator<Item = Predicate<E>>) -> Predicate<E> {
        Predicate::Or(parts.into_iter().collect())
    }

    pub fn not(&self, inner: Predicate<E>) -> Predicate<E> {
        Predicate::Not(Box::new(inner))
    }
}
