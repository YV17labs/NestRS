//! Single condition AST shared by the query pre-filter
//! ([`Predicate::to_condition`] → SQL `WHERE`) and the response check
//! ([`Predicate::matches`] → in-memory). Sharing one AST keeps the rows a
//! query returns and the rows the response check accepts from drifting apart.

use std::cmp::Ordering;
use std::marker::PhantomData;

use sea_orm::sea_query::{Condition, DynIden, Expr, ExprTrait, Query};
use sea_orm::{
    ColumnTrait, EntityTrait, Identity, ModelTrait, RelationDef, RelationTrait, RelationType, Value,
};

/// Scalar comparison operator for [`Predicate::Cmp`]. Equality already has its
/// own variant ([`Predicate::Eq`]); this covers the remaining ordered/inequality
/// comparisons. Deliberately minimal — extend on demand, not speculatively.
#[derive(Clone, Copy, Debug)]
pub enum CmpOp {
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
}

/// A condition over entity `E`, interpreted as SQL or in memory.
#[derive(Default)]
pub enum Predicate<E: EntityTrait> {
    #[default]
    Always,
    /// Malformation sentinel: renders an always-false SQL condition (`1 = 0`)
    /// and matches no row in memory. Produced when [`PredicateBuilder::related`]
    /// rejects an invalid relation predicate (composite key or a relation not
    /// pointing at `R`). A rule carrying this sentinel fails
    /// [`AbilityBuilder::build`](crate::AbilityBuilder::build) with a
    /// [`MalformedRuleError`](crate::MalformedRuleError) — the misconfiguration
    /// surfaces loudly at ability construction. This matters most on the
    /// *denial* side: a `cannot(...)` lowering to `1 = 0` would combine as
    /// `grant AND NOT(1 = 0)`, i.e. fail-*open*; failing construction closes
    /// that hole. On the grant side the same sentinel is fail-closed (deny-all),
    /// but a silent developer error is still worth surfacing.
    Deny,
    Eq(E::Column, Value),
    In(E::Column, Vec<Value>),
    /// Scalar comparison (`Ne`/`Lt`/`Lte`/`Gt`/`Gte`) on one of `E`'s own
    /// columns. Pure and symmetric: SQL renders the matching operator, the
    /// in-memory check orders the model's value against the bound value.
    Cmp(E::Column, CmpOp, Value),
    /// Nullness test on one of `E`'s own columns. `true` → `IS NULL`,
    /// `false` → `IS NOT NULL`.
    IsNull(E::Column, bool),
    And(Vec<Predicate<E>>),
    Or(Vec<Predicate<E>>),
    Not(Box<Predicate<E>>),
    /// Scope `E` by a condition on a *related* entity reached through a typed
    /// SeaORM relation. Type-erased over the related entity (see
    /// [`RelatedPredicate`]).
    Related(Box<RelatedPredicate>),
}

/// A condition that scopes `E` by a sub-condition on a *related* entity `R`,
/// reached through a typed SeaORM relation. Type-erased over `R`: the
/// sub-condition is pre-lowered to SQL (over `R`'s columns) at rule-definition
/// time, and the join metadata comes from the [`RelationDef`]. Nothing here is
/// generic over `R`, so it lives inside the monomorphic [`Predicate<E>`] without
/// a type parameter.
///
/// The in-memory interpreter cannot traverse the relation without loading the
/// parent, so [`Predicate::matches`] defers to SQL for relational row
/// visibility (returns `true`); the by-id / create paths re-check against the
/// same lowered condition in SQL instead (one source of truth).
#[derive(Clone)]
pub struct RelatedPredicate {
    /// `E -> R` relation: yields the from-column (on `E`), the to-column (on
    /// `R`), both tables, and the relation type.
    relation: RelationDef,
    /// `R`'s filter, already lowered to SQL over `R`'s own columns.
    sub_condition: Condition,
}

impl RelatedPredicate {
    /// Lower to a semi-join against the related table. `belongs_to`/`has_one`
    /// ([`RelationType::HasOne`]) becomes an uncorrelated `IN (subquery)`;
    /// `has_many` becomes a correlated `EXISTS`. A relation's custom
    /// `on_condition` is folded into the subquery's `WHERE` so the scope is not
    /// widened past the join.
    fn to_condition(&self) -> Condition {
        let def = &self.relation;
        // v1 supports single-column keys only. A composite key is rejected in
        // `PredicateBuilder::related` (which returns `Predicate::Deny` rather
        // than building a `Related`), so this path is unreachable in practice;
        // it stays as fail-closed defense-in-depth — deny every row rather than
        // panic the request.
        let (Some(from_col), Some(to_col)) = (unary_iden(&def.from_col), unary_iden(&def.to_col))
        else {
            tracing::error!(
                target: "nest_rs::authz",
                reason = "composite_key",
                "invalid ability relation predicate — denying all rows",
            );
            return Condition::all().add(Expr::cust("1 = 0"));
        };
        let from_tbl = def.from_tbl.sea_orm_table().clone();
        let to_tbl = def.to_tbl.sea_orm_table().clone();

        let mut where_cond = self.sub_condition.clone();
        if let Some(on) = &def.on_condition {
            where_cond = where_cond.add(on(from_tbl.clone(), to_tbl.clone()));
        }

        match def.rel_type {
            RelationType::HasOne => {
                let subquery = Query::select()
                    .expr(Expr::col((to_tbl, to_col)))
                    .from(def.to_tbl.clone())
                    .cond_where(where_cond)
                    .take();
                Condition::all().add(Expr::col((from_tbl, from_col)).in_subquery(subquery))
            }
            RelationType::HasMany => {
                let correlated = Condition::all()
                    .add(Expr::col((to_tbl, to_col)).equals((from_tbl, from_col)))
                    .add(where_cond);
                let subquery = Query::select()
                    .expr(Expr::val(1))
                    .from(def.to_tbl.clone())
                    .cond_where(correlated)
                    .take();
                Condition::all().add(Expr::exists(subquery))
            }
        }
    }
}

/// The single column of an [`Identity`], or `None` for a composite key.
fn unary_iden(id: &Identity) -> Option<DynIden> {
    match id {
        Identity::Unary(col) => Some(col.clone()),
        _ => None,
    }
}

impl<E: EntityTrait> Predicate<E> {
    /// Lower to a [`sea_orm::Condition`]. `Condition::all()` is SQL `TRUE`, so
    /// [`Predicate::Always`] imposes no constraint.
    pub fn to_condition(&self) -> Condition {
        match self {
            Predicate::Always => Condition::all(),
            // Fail-closed: `Condition::all().add(Expr::cust("1 = 0"))` is the
            // same always-false clause `Repo::scope_for` renders on a denied
            // request (`nest-rs-seaorm/src/repo.rs`).
            Predicate::Deny => Condition::all().add(Expr::cust("1 = 0")),
            Predicate::Eq(col, value) => Condition::all().add(col.eq(value.clone())),
            Predicate::In(col, values) => Condition::all().add(col.is_in(values.clone())),
            Predicate::Cmp(col, op, value) => {
                let v = value.clone();
                let expr = match op {
                    CmpOp::Ne => col.ne(v),
                    CmpOp::Lt => col.lt(v),
                    CmpOp::Lte => col.lte(v),
                    CmpOp::Gt => col.gt(v),
                    CmpOp::Gte => col.gte(v),
                };
                Condition::all().add(expr)
            }
            Predicate::IsNull(col, is_null) => {
                let expr = if *is_null {
                    col.is_null()
                } else {
                    col.is_not_null()
                };
                Condition::all().add(expr)
            }
            Predicate::And(parts) => parts
                .iter()
                .fold(Condition::all(), |acc, p| acc.add(p.to_condition())),
            Predicate::Or(parts) => parts
                .iter()
                .fold(Condition::any(), |acc, p| acc.add(p.to_condition())),
            Predicate::Not(inner) => inner.to_condition().not(),
            Predicate::Related(rp) => rp.to_condition(),
        }
    }

    /// Reads each column with [`ModelTrait::get`], which returns the same
    /// [`Value`] the SQL side compares against — so the in-memory check and
    /// the SQL filter cannot disagree.
    pub fn matches(&self, model: &E::Model) -> bool {
        match self {
            Predicate::Always => true,
            // Fail-closed: a denied predicate accepts no row.
            Predicate::Deny => false,
            Predicate::Eq(col, value) => &model.get(*col) == value,
            Predicate::In(col, values) => {
                let actual = model.get(*col);
                values.iter().any(|v| v == &actual)
            }
            Predicate::Cmp(col, op, value) => {
                let actual = model.get(*col);
                match op {
                    // `Value: PartialEq`, so inequality needs no ordering.
                    CmpOp::Ne => &actual != value,
                    // Ordered comparisons fail closed when the two values are
                    // not orderable (mismatched variants, or a NULL column):
                    // an undecidable comparison never reports a match.
                    _ => match value_ordering(&actual, value) {
                        Some(ord) => match op {
                            CmpOp::Lt => ord == Ordering::Less,
                            CmpOp::Lte => ord != Ordering::Greater,
                            CmpOp::Gt => ord == Ordering::Greater,
                            CmpOp::Gte => ord != Ordering::Less,
                            CmpOp::Ne => unreachable!("Ne handled above"),
                        },
                        None => false,
                    },
                }
            }
            Predicate::IsNull(col, want_null) => model.get(*col).is_some() != *want_null,
            Predicate::And(parts) => parts.iter().all(|p| p.matches(model)),
            Predicate::Or(parts) => parts.iter().any(|p| p.matches(model)),
            Predicate::Not(inner) => !inner.matches(model),
            // Row visibility for a relational rule is decided in SQL (system 2):
            // the list filter already excluded out-of-scope rows, and the
            // by-id/create paths re-check against the same lowered condition.
            // The in-memory check cannot traverse the relation without loading
            // the parent (it would have to be async and would load rows on the
            // fly), so it defers rather than guessing. Field masking still runs.
            Predicate::Related(_) => true,
        }
    }
}

/// Order two [`Value`]s of the same variant. `Value` derives `PartialEq` but not
/// `PartialOrd`, so the ordered `Cmp` operators need this. Returns `None` for
/// `NULL` operands or mismatched variants — the in-memory `Cmp` arm treats that
/// as "no match" (fail closed). Covers the scalar and temporal column types a
/// row-level rule realistically orders; equality/`Ne` does not route through here.
fn value_ordering(a: &Value, b: &Value) -> Option<Ordering> {
    use Value::*;
    match (a, b) {
        (Bool(Some(x)), Bool(Some(y))) => x.partial_cmp(y),
        (TinyInt(Some(x)), TinyInt(Some(y))) => x.partial_cmp(y),
        (SmallInt(Some(x)), SmallInt(Some(y))) => x.partial_cmp(y),
        (Int(Some(x)), Int(Some(y))) => x.partial_cmp(y),
        (BigInt(Some(x)), BigInt(Some(y))) => x.partial_cmp(y),
        (TinyUnsigned(Some(x)), TinyUnsigned(Some(y))) => x.partial_cmp(y),
        (SmallUnsigned(Some(x)), SmallUnsigned(Some(y))) => x.partial_cmp(y),
        (Unsigned(Some(x)), Unsigned(Some(y))) => x.partial_cmp(y),
        (BigUnsigned(Some(x)), BigUnsigned(Some(y))) => x.partial_cmp(y),
        (Float(Some(x)), Float(Some(y))) => x.partial_cmp(y),
        (Double(Some(x)), Double(Some(y))) => x.partial_cmp(y),
        (String(Some(x)), String(Some(y))) => x.partial_cmp(y),
        (Char(Some(x)), Char(Some(y))) => x.partial_cmp(y),
        (Uuid(Some(x)), Uuid(Some(y))) => x.partial_cmp(y),
        (ChronoDate(Some(x)), ChronoDate(Some(y))) => x.partial_cmp(y),
        (ChronoTime(Some(x)), ChronoTime(Some(y))) => x.partial_cmp(y),
        (ChronoDateTime(Some(x)), ChronoDateTime(Some(y))) => x.partial_cmp(y),
        (ChronoDateTimeUtc(Some(x)), ChronoDateTimeUtc(Some(y))) => x.partial_cmp(y),
        (ChronoDateTimeLocal(Some(x)), ChronoDateTimeLocal(Some(y))) => x.partial_cmp(y),
        (ChronoDateTimeWithTimeZone(Some(x)), ChronoDateTimeWithTimeZone(Some(y))) => {
            x.partial_cmp(y)
        }
        _ => None,
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

    pub fn ne(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Ne, value.into())
    }

    pub fn lt(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Lt, value.into())
    }

    pub fn lte(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Lte, value.into())
    }

    pub fn gt(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Gt, value.into())
    }

    pub fn gte(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Gte, value.into())
    }

    /// `col IS NULL`.
    pub fn is_null(&self, column: E::Column) -> Predicate<E> {
        Predicate::IsNull(column, true)
    }

    /// `col IS NOT NULL`.
    pub fn is_not_null(&self, column: E::Column) -> Predicate<E> {
        Predicate::IsNull(column, false)
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

    /// Scope `E` by a condition on a related entity `R`, reached via `relation`
    /// (a variant of `E`'s SeaORM `Relation` enum). The closure builds the
    /// sub-condition using `R`'s own typed columns.
    ///
    /// ```ignore
    /// // `message` has no `org_id`; its tenant is the parent conversation's.
    /// ab.can(Action::Read, message::Entity).when(|p| {
    ///     p.related::<conversation::Entity, _>(
    ///         message::Relation::Conversation,
    ///         |c| c.eq(conversation::Column::OrgId, org_id),
    ///     )
    /// });
    /// ```
    ///
    /// Two runtime checks close the gap that type erasure opens. `related()`
    /// runs inside `AbilityFactory::define` — i.e. per authenticated request —
    /// so a violation never panics the request: each check logs an `error!`
    /// (target `nest_rs::authz`) and returns [`Predicate::Deny`]. A rule that
    /// carries that sentinel then fails
    /// [`AbilityBuilder::build`](crate::AbilityBuilder::build), so the
    /// misconfiguration is rejected loudly rather than silently going fail-open
    /// on the denial side. Both cases are a developer misconfiguration:
    /// - the relation must point at `R` (its `to_tbl` must be `R`'s table);
    /// - the relation must be single-column (composite keys are unsupported in
    ///   v1).
    pub fn related<R, F>(&self, relation: E::Relation, build: F) -> Predicate<E>
    where
        R: EntityTrait,
        F: FnOnce(PredicateBuilder<R>) -> Predicate<R>,
    {
        let def = relation.def();
        let expected = R::default().table_ref();
        if def.to_tbl.sea_orm_table() != expected.sea_orm_table() {
            tracing::error!(
                target: "nest_rs::authz",
                related = std::any::type_name::<R>(),
                reason = "relation_table_mismatch",
                "invalid ability relation predicate — denying all rows",
            );
            return Predicate::Deny;
        }
        if def.from_col.arity() != 1 || def.to_col.arity() != 1 {
            tracing::error!(
                target: "nest_rs::authz",
                related = std::any::type_name::<R>(),
                reason = "composite_key",
                "invalid ability relation predicate — denying all rows",
            );
            return Predicate::Deny;
        }
        let sub = build(PredicateBuilder::<R>::new());
        Predicate::Related(Box::new(RelatedPredicate {
            relation: def,
            sub_condition: sub.to_condition(),
        }))
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
            // Nullable column so `IsNull` and `Cmp`-on-NULL have something to bite on.
            pub tag: Option<String>,
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
            tag: None,
        }
    }

    fn tagged(id: i32, org: i32, name: &str, tag: Option<&str>) -> widget::Model {
        widget::Model {
            id,
            org_id: org,
            name: name.into(),
            tag: tag.map(Into::into),
        }
    }

    // A parent/child pair for relational tests: `child` belongs_to `parent`,
    // and the tenant key (`org_id`) lives only on the parent — exactly the
    // shape the relational filter exists for.
    mod parent {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "parents")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod child {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "children")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub parent_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {
            #[sea_orm(
                belongs_to = "super::parent::Entity",
                from = "Column::ParentId",
                to = "super::parent::Column::Id"
            )]
            Parent,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

    fn child_sql(p: &Predicate<child::Entity>) -> String {
        child::Entity::find()
            .filter(p.to_condition())
            .build(DatabaseBackend::Postgres)
            .to_string()
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
    fn ne_excludes_the_bound_value_in_memory_and_sql() {
        let p = b().ne(widget::Column::OrgId, 7);
        assert!(!p.matches(&model(1, 7, "a")));
        assert!(p.matches(&model(1, 8, "a")));
        let s = sql(&p);
        assert!(s.contains("org_id"), "Ne must mention column: {s}");
        assert!(s.contains("<>"), "Ne must compile to SQL <>: {s}");
    }

    #[test]
    fn lt_and_lte_order_by_column_value() {
        let lt = b().lt(widget::Column::OrgId, 5);
        assert!(lt.matches(&model(1, 4, "a")));
        assert!(!lt.matches(&model(1, 5, "a")));
        assert!(!lt.matches(&model(1, 6, "a")));
        assert!(sql(&lt).contains('<'), "Lt renders <: {}", sql(&lt));

        let lte = b().lte(widget::Column::OrgId, 5);
        assert!(lte.matches(&model(1, 4, "a")));
        assert!(lte.matches(&model(1, 5, "a")));
        assert!(!lte.matches(&model(1, 6, "a")));
        assert!(sql(&lte).contains("<="), "Lte renders <=: {}", sql(&lte));
    }

    #[test]
    fn gt_and_gte_order_by_column_value() {
        let gt = b().gt(widget::Column::OrgId, 5);
        assert!(!gt.matches(&model(1, 5, "a")));
        assert!(gt.matches(&model(1, 6, "a")));
        assert!(sql(&gt).contains('>'), "Gt renders >: {}", sql(&gt));

        let gte = b().gte(widget::Column::OrgId, 5);
        assert!(gte.matches(&model(1, 5, "a")));
        assert!(!gte.matches(&model(1, 4, "a")));
        assert!(sql(&gte).contains(">="), "Gte renders >=: {}", sql(&gte));
    }

    #[test]
    fn ordered_cmp_on_a_null_column_fails_closed() {
        // `tag IS NULL`, so the ordered comparison is undecidable — the
        // in-memory check must report no match, never silently admit the row.
        let p = b().lt(widget::Column::Tag, "m");
        assert!(!p.matches(&tagged(1, 7, "a", None)));
        // A present value still orders normally.
        assert!(p.matches(&tagged(1, 7, "a", Some("abc"))));
        assert!(!p.matches(&tagged(1, 7, "a", Some("zzz"))));
    }

    #[test]
    fn is_null_matches_absent_values_in_memory_and_sql() {
        let p = b().is_null(widget::Column::Tag);
        assert!(p.matches(&tagged(1, 7, "a", None)));
        assert!(!p.matches(&tagged(1, 7, "a", Some("x"))));
        let s = sql(&p);
        assert!(s.contains("tag"), "IsNull must mention column: {s}");
        assert!(
            s.to_uppercase().contains("IS NULL"),
            "IsNull must render IS NULL: {s}"
        );
    }

    #[test]
    fn is_not_null_matches_present_values_in_memory_and_sql() {
        let p = b().is_not_null(widget::Column::Tag);
        assert!(p.matches(&tagged(1, 7, "a", Some("x"))));
        assert!(!p.matches(&tagged(1, 7, "a", None)));
        let s = sql(&p);
        assert!(
            s.to_uppercase().contains("IS NOT NULL"),
            "IsNull(false) must render IS NOT NULL: {s}"
        );
    }

    #[test]
    fn related_belongs_to_lowers_to_in_subquery() {
        // `child` scoped by `parent.org_id` (child has no org_id column).
        let p = PredicateBuilder::<child::Entity>::new()
            .related::<parent::Entity, _>(child::Relation::Parent, |c| {
                c.eq(parent::Column::OrgId, 7)
            });
        let s = child_sql(&p);
        assert!(
            s.contains("IN (SELECT"),
            "belongs_to must lower to an IN subquery: {s}"
        );
        assert!(
            s.contains("parents"),
            "subquery must select from the parent table: {s}"
        );
        assert!(
            s.contains("org_id"),
            "subquery must filter the parent's column: {s}"
        );
        assert!(
            s.contains("parent_id"),
            "outer query must constrain the child FK: {s}"
        );
    }

    #[test]
    fn related_defers_row_visibility_to_sql_in_memory() {
        // The in-memory check cannot traverse the relation, so it returns true
        // and lets the SQL filter (and the by-id/create re-check) decide.
        let p = PredicateBuilder::<child::Entity>::new()
            .related::<parent::Entity, _>(child::Relation::Parent, |c| {
                c.eq(parent::Column::OrgId, 7)
            });
        assert!(p.matches(&child::Model {
            id: 1,
            parent_id: 99
        }));
        assert!(p.matches(&child::Model {
            id: 2,
            parent_id: 1
        }));
    }

    #[test]
    fn related_has_many_lowers_to_correlated_exists() {
        use sea_orm::RelationTrait;
        // Fabricate the inverse `parent -> children` has_many relation from the
        // child's belongs_to def (rev keeps the type, so flip it to HasMany).
        let mut def = child::Relation::Parent.def().rev();
        def.rel_type = sea_orm::RelationType::HasMany;
        let sub = PredicateBuilder::<child::Entity>::new().eq(child::Column::Id, 5);
        let rp = RelatedPredicate {
            relation: def,
            sub_condition: sub.to_condition(),
        };
        let s = parent::Entity::find()
            .filter(rp.to_condition())
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(
            s.to_uppercase().contains("EXISTS(SELECT"),
            "has_many must lower to a correlated EXISTS: {s}"
        );
        assert!(
            s.contains("children") && s.contains("parents"),
            "EXISTS must correlate the child table back to the parent: {s}"
        );
    }

    #[test]
    fn related_rejects_a_relation_not_pointing_at_r() {
        // The relation targets `parent`, but R is declared as `child` — the
        // mismatch type erasure would hide must be rejected fail-closed
        // (deny-all), not panic the per-request `AbilityFactory::define`.
        let p = PredicateBuilder::<child::Entity>::new()
            .related::<child::Entity, _>(child::Relation::Parent, |c| c.eq(child::Column::Id, 1));
        assert!(matches!(p, Predicate::Deny));
        let s = child_sql(&p);
        assert!(
            s.contains("1 = 0"),
            "rejected relation must render always-false SQL: {s}"
        );
    }

    #[test]
    fn deny_matches_nothing_in_memory_and_renders_always_false_sql() {
        let p: Predicate<widget::Entity> = Predicate::Deny;
        assert!(!p.matches(&model(1, 7, "a")));
        assert!(!p.matches(&model(99, 0, "")));
        let s = sql(&p);
        assert!(
            s.contains("1 = 0"),
            "Deny must render always-false SQL: {s}"
        );
    }

    #[test]
    fn default_predicate_is_always() {
        let p: Predicate<widget::Entity> = Default::default();
        assert!(matches!(p, Predicate::Always));
    }
}
