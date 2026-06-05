//! Single condition AST shared by the query pre-filter
//! ([`Predicate::to_condition`] → SQL `WHERE`) and the response check
//! ([`Predicate::matches`] → in-memory). Sharing one AST keeps the rows a
//! query returns and the rows the response check accepts from drifting apart.

use std::marker::PhantomData;

use sea_orm::sea_query::Condition;
use sea_orm::{ColumnTrait, EntityTrait, ModelTrait, Value};

/// A condition over entity `E`, interpreted as SQL or in memory.
#[derive(Default)]
pub enum Predicate<E: EntityTrait> {
    #[default]
    Always,
    Eq(E::Column, Value),
    In(E::Column, Vec<Value>),
    And(Vec<Predicate<E>>),
    Or(Vec<Predicate<E>>),
    Not(Box<Predicate<E>>),
}

impl<E: EntityTrait> Predicate<E> {
    /// Lower to a [`sea_orm::Condition`]. `Condition::all()` is SQL `TRUE`, so
    /// [`Predicate::Always`] imposes no constraint.
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

    /// Reads each column with [`ModelTrait::get`], which returns the same
    /// [`Value`] the SQL side compares against — so the in-memory check and
    /// the SQL filter cannot disagree.
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

/// Handed to a rule's `when(|p| …)` closure so the condition reads
/// `p.eq(Column::OrgId, actor.org_id)`.
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

#[cfg(test)]
mod tests {
    use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait};

    use super::*;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    fn model(id: i32, org: i32, name: &str) -> widget::Model {
        widget::Model {
            id,
            org_id: org,
            name: name.into(),
        }
    }

    fn sql(p: &Predicate<widget::Entity>) -> String {
        widget::Entity::find()
            .filter(p.to_condition())
            .build(DatabaseBackend::Postgres)
            .to_string()
    }

    fn b() -> PredicateBuilder<widget::Entity> {
        PredicateBuilder::new()
    }

    // The SQL pre-filter and the in-memory check must agree row-by-row —
    // tests pin both branches per variant on the same models so a future
    // refactor of one can't silently diverge from the other.

    #[test]
    fn always_matches_every_row_in_memory_and_renders_unconstrained_sql() {
        let p: Predicate<widget::Entity> = Predicate::Always;
        assert!(p.matches(&model(1, 7, "a")));
        assert!(p.matches(&model(99, 0, "")));
        let s = sql(&p);
        assert!(!s.contains("1 = 0"), "Always must not constrain: {s}");
    }

    #[test]
    fn eq_filters_by_column_value_in_memory_and_sql() {
        let p = b().eq(widget::Column::OrgId, 7);
        assert!(p.matches(&model(1, 7, "a")));
        assert!(!p.matches(&model(1, 8, "a")));
        let s = sql(&p);
        assert!(s.contains("org_id"), "Eq must mention column: {s}");
    }

    #[test]
    fn is_in_admits_any_listed_value() {
        let p = b().is_in(widget::Column::Id, [1i32, 2, 3]);
        assert!(p.matches(&model(1, 7, "a")));
        assert!(p.matches(&model(3, 7, "a")));
        assert!(!p.matches(&model(4, 7, "a")));
        let s = sql(&p);
        assert!(s.contains(" IN "), "is_in must compile to SQL IN: {s}");
    }

    #[test]
    fn is_in_with_empty_list_matches_nothing_in_memory() {
        // SeaORM's empty `IS IN ()` renders to a falsy condition; the in-memory
        // check returns false to match.
        let p: Predicate<widget::Entity> = b().is_in(widget::Column::Id, Vec::<i32>::new());
        assert!(!p.matches(&model(1, 7, "a")));
    }

    #[test]
    fn and_requires_every_part_to_match() {
        let p = b().all([
            b().eq(widget::Column::OrgId, 7),
            b().eq(widget::Column::Name, "ada"),
        ]);
        assert!(p.matches(&model(1, 7, "ada")));
        assert!(!p.matches(&model(1, 7, "bob")));
        assert!(!p.matches(&model(1, 8, "ada")));
    }

    #[test]
    fn empty_and_is_vacuously_true() {
        // `all([])` mirrors `Always` — folds to `Condition::all()`.
        let p: Predicate<widget::Entity> = b().all(Vec::new());
        assert!(p.matches(&model(1, 7, "a")));
    }

    #[test]
    fn or_requires_one_part_to_match() {
        let p = b().any([
            b().eq(widget::Column::OrgId, 7),
            b().eq(widget::Column::Name, "ada"),
        ]);
        assert!(p.matches(&model(1, 7, "bob")));
        assert!(p.matches(&model(1, 9, "ada")));
        assert!(!p.matches(&model(1, 9, "bob")));
    }

    #[test]
    fn empty_or_matches_nothing() {
        // `any([])` folds to `Condition::any()` — SQL FALSE.
        let p: Predicate<widget::Entity> = b().any(Vec::new());
        assert!(!p.matches(&model(1, 7, "a")));
    }

    #[test]
    fn not_inverts_membership() {
        let inner = b().eq(widget::Column::OrgId, 7);
        let p = b().not(inner);
        assert!(!p.matches(&model(1, 7, "a")));
        assert!(p.matches(&model(1, 8, "a")));
        let s = sql(&p);
        assert!(s.to_uppercase().contains("NOT"), "NOT must appear: {s}");
    }

    #[test]
    fn double_negation_is_idempotent_in_memory() {
        let inner = b().eq(widget::Column::Id, 42);
        let p = b().not(b().not(inner));
        assert!(p.matches(&model(42, 7, "a")));
        assert!(!p.matches(&model(43, 7, "a")));
    }

    #[test]
    fn nested_and_or_compose_as_expected() {
        // (org = 7 AND name = "ada") OR id = 999
        let p = b().any([
            b().all([
                b().eq(widget::Column::OrgId, 7),
                b().eq(widget::Column::Name, "ada"),
            ]),
            b().eq(widget::Column::Id, 999),
        ]);
        assert!(p.matches(&model(1, 7, "ada")));
        assert!(p.matches(&model(999, 0, "anyone")));
        assert!(!p.matches(&model(1, 7, "bob")));
        assert!(!p.matches(&model(998, 8, "ada")));
    }

    #[test]
    fn default_predicate_is_always() {
        let p: Predicate<widget::Entity> = Default::default();
        assert!(matches!(p, Predicate::Always));
    }
}
