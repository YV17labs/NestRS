//! Single condition AST shared by the query pre-filter
//! ([`Predicate::to_condition`] → SQL `WHERE`) and the response check
//! ([`Predicate::matches`] → in-memory).
//!
//! The in-memory side answers only what the row in hand shows, as the SQL
//! `to_condition` renders would: an `eq` or `ne` against `NULL` is `IS NULL` or
//! `IS NOT NULL`, as sea-orm renders it, and any other comparison with `NULL`
//! is `UNKNOWN`, SQL's third truth value. What the row cannot show at all — a
//! relation, a column the wire never carried, two value types only the
//! database compares — is left undecided, and the mask says what that means
//! for the row.

use std::cmp::Ordering;
use std::marker::PhantomData;

use sea_orm::sea_query::{Condition, DynIden, Expr, ExprTrait, Query};
use sea_orm::{
    ColumnTrait, EntityTrait, Identity, ModelTrait, RelationDef, RelationTrait, RelationType, Value,
};

/// Scalar comparison operator for [`Predicate::Cmp`]; equality is [`Predicate::Eq`].
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
    /// Unconditional match — an unrestricted grant/denial over every row.
    #[default]
    Always,
    /// Malformation sentinel: renders `1 = 0` and matches no row in memory.
    /// Produced when [`PredicateBuilder::related`] rejects a relation; a rule
    /// carrying it fails [`AbilityBuilder::build`](crate::AbilityBuilder::build).
    Deny,
    /// Column equals a bound value — `col = value`.
    Eq(E::Column, Value),
    /// Column is one of a set — `col IN (values)`.
    In(E::Column, Vec<Value>),
    /// Scalar comparison (`Ne`/`Lt`/`Lte`/`Gt`/`Gte`) on one of `E`'s own
    /// columns. Pure and symmetric: SQL renders the matching operator, the
    /// in-memory check orders the model's value against the bound value.
    Cmp(E::Column, CmpOp, Value),
    /// Nullness test on one of `E`'s own columns. `true` → `IS NULL`,
    /// `false` → `IS NOT NULL`.
    IsNull(E::Column, bool),
    /// Conjunction — every sub-predicate must match.
    And(Vec<Predicate<E>>),
    /// Disjunction — at least one sub-predicate must match.
    Or(Vec<Predicate<E>>),
    /// Negation of the inner predicate.
    Not(Box<Predicate<E>>),
    /// Scope `E` by a condition on a *related* entity reached through a typed
    /// SeaORM relation. Type-erased over the related entity (see
    /// `RelatedPredicate`).
    Related(Box<RelatedPredicate>),
}

/// A condition that scopes `E` by a sub-condition on a *related* entity `R`,
/// reached through a typed SeaORM relation. Type-erased over `R`: the
/// sub-condition is pre-lowered to SQL (over `R`'s columns) at rule-definition
/// time, and the join metadata comes from the [`RelationDef`].
///
/// The in-memory check cannot traverse the relation without loading the
/// parent, so it leaves the rule to SQL.
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
        // Unreachable: `PredicateBuilder::related` rejects a composite key. Fail
        // closed rather than panic the request.
        let (Some(from_col), Some(to_col)) = (unary_iden(&def.from_col), unary_iden(&def.to_col))
        else {
            tracing::error!(
                target: crate::TARGET,
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

    /// Whether SQL's filter certainly admits this row, read off its own columns.
    ///
    /// Each column is read with [`ModelTrait::get`], which returns the same
    /// [`Value`] the SQL side compares against. A relation, or a comparison
    /// between two value types only the database can compare, is not decided
    /// here and reads `false`; [`Ability::can`](crate::Ability::can) is the
    /// instance check, which takes a grant's relation to hold.
    pub fn matches(&self, model: &E::Model) -> bool {
        self.decide(model, &nothing_invented) == Truth::True
    }

    /// What this predicate is on `model`, as SQL would compute it from the row's
    /// columns. `invented` names the columns whose value the row was given
    /// rather than read, so a rule on one is undecided.
    pub(crate) fn decide(&self, model: &E::Model, invented: &dyn Fn(E::Column) -> bool) -> Truth {
        self.judge(model, invented, Truth::Undecidable)
    }

    /// [`decide`](Self::decide) on a row read whole, with every relation taken
    /// to hold: how the instance check reads a grant, leaving a relation to the
    /// read that produced the row.
    pub(crate) fn decide_holding_relations(&self, model: &E::Model) -> Truth {
        self.judge(model, &nothing_invented, Truth::True)
    }

    /// The one walk behind both readings; `relation` is what a relation is.
    fn judge(
        &self,
        model: &E::Model,
        invented: &dyn Fn(E::Column) -> bool,
        relation: Truth,
    ) -> Truth {
        match self {
            Predicate::Always => Truth::True,
            // Fail-closed: a denied predicate accepts no row.
            Predicate::Deny => Truth::False,
            Predicate::Related(_) => relation,
            Predicate::Eq(col, _)
            | Predicate::In(col, _)
            | Predicate::Cmp(col, _, _)
            | Predicate::IsNull(col, _)
                if invented(*col) =>
            {
                Truth::Undecidable
            }
            // sea-orm renders `eq` and `ne` against a `NULL` value as `IS NULL`
            // and `IS NOT NULL` (`ColumnTrait::eq`), never as `= NULL`.
            Predicate::Eq(col, value) if !value.is_some() => Truth::of(!model.get(*col).is_some()),
            Predicate::Cmp(col, CmpOp::Ne, value) if !value.is_some() => {
                Truth::of(model.get(*col).is_some())
            }
            Predicate::Eq(col, value) => compare(&model.get(*col), value, |a, b| Some(a == b)),
            Predicate::In(col, values) => one_of(&model.get(*col), values),
            Predicate::Cmp(col, op, value) => compare(&model.get(*col), value, |a, b| match op {
                // `Value: PartialEq`, so inequality needs no ordering.
                CmpOp::Ne => Some(a != b),
                CmpOp::Lt => value_ordering(a, b).map(Ordering::is_lt),
                CmpOp::Lte => value_ordering(a, b).map(Ordering::is_le),
                CmpOp::Gt => value_ordering(a, b).map(Ordering::is_gt),
                CmpOp::Gte => value_ordering(a, b).map(Ordering::is_ge),
            }),
            Predicate::IsNull(col, want_null) => Truth::of(model.get(*col).is_some() != *want_null),
            Predicate::And(parts) => {
                Truth::all(parts.iter().map(|p| p.judge(model, invented, relation)))
            }
            Predicate::Or(parts) => {
                Truth::any(parts.iter().map(|p| p.judge(model, invented, relation)))
            }
            Predicate::Not(inner) => inner.judge(model, invented, relation).not(),
        }
    }
}

/// What a predicate is on one row, as far as the row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Truth {
    /// SQL's `TRUE`: the filter admits the row.
    True,
    /// SQL's `FALSE`.
    False,
    /// SQL's `UNKNOWN`, from a comparison with `NULL`: the filter admits the
    /// row neither under it nor under its negation.
    Unknown,
    /// The row cannot show it: a relation, an invented column, or two value
    /// types only the database compares.
    Undecidable,
}

impl Truth {
    fn of(holds: bool) -> Self {
        if holds { Truth::True } else { Truth::False }
    }

    fn not(self) -> Self {
        match self {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            other => other,
        }
    }

    /// SQL's `AND`: one `FALSE` decides it whatever the others are.
    fn all(parts: impl Iterator<Item = Truth>) -> Self {
        let mut out = Truth::True;
        for part in parts {
            match part {
                Truth::False => return Truth::False,
                Truth::Undecidable => out = Truth::Undecidable,
                Truth::Unknown if out == Truth::True => out = Truth::Unknown,
                _ => {}
            }
        }
        out
    }

    /// SQL's `OR`: one `TRUE` decides it whatever the others are.
    fn any(parts: impl Iterator<Item = Truth>) -> Self {
        let mut out = Truth::False;
        for part in parts {
            match part {
                Truth::True => return Truth::True,
                Truth::Undecidable => out = Truth::Undecidable,
                Truth::Unknown if out == Truth::False => out = Truth::Unknown,
                _ => {}
            }
        }
        out
    }
}

/// The `invented` of a row read whole: every column holds what was read.
pub(crate) fn nothing_invented<C>(_column: C) -> bool {
    false
}

/// SQL's verdict on `actual <op> bound` for a bound the SQL side binds as a
/// value: `UNKNOWN` with a `NULL` operand, and undecided across two value
/// types, or where `test` cannot order the two.
fn compare(
    actual: &Value,
    bound: &Value,
    test: impl FnOnce(&Value, &Value) -> Option<bool>,
) -> Truth {
    if !actual.is_some() || !bound.is_some() {
        return Truth::Unknown;
    }
    if std::mem::discriminant(actual) != std::mem::discriminant(bound) {
        return Truth::Undecidable;
    }
    test(actual, bound).map_or(Truth::Undecidable, Truth::of)
}

/// SQL's `actual IN (values)`: an empty list matches nothing, and a list that
/// misses the value but holds a `NULL` is `UNKNOWN`.
fn one_of(actual: &Value, values: &[Value]) -> Truth {
    Truth::any(
        values
            .iter()
            .map(|value| compare(actual, value, |a, b| Some(a == b))),
    )
}

/// Order two [`Value`]s of the same variant (`Value` is not `PartialOrd`);
/// `None` for a variant it cannot order, which leaves the comparison undecided.
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

    /// `column = value`.
    pub fn eq(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Eq(column, value.into())
    }

    /// `column IN (values)`.
    pub fn is_in<V: Into<Value>>(
        &self,
        column: E::Column,
        values: impl IntoIterator<Item = V>,
    ) -> Predicate<E> {
        Predicate::In(column, values.into_iter().map(Into::into).collect())
    }

    /// `column <> value`.
    pub fn ne(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Ne, value.into())
    }

    /// `column < value`.
    pub fn lt(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Lt, value.into())
    }

    /// `column <= value`.
    pub fn lte(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Lte, value.into())
    }

    /// `column > value`.
    pub fn gt(&self, column: E::Column, value: impl Into<Value>) -> Predicate<E> {
        Predicate::Cmp(column, CmpOp::Gt, value.into())
    }

    /// `column >= value`.
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

    /// Conjunction — matches when every `part` matches.
    pub fn all(&self, parts: impl IntoIterator<Item = Predicate<E>>) -> Predicate<E> {
        Predicate::And(parts.into_iter().collect())
    }

    /// Disjunction — matches when any `part` matches.
    pub fn any(&self, parts: impl IntoIterator<Item = Predicate<E>>) -> Predicate<E> {
        Predicate::Or(parts.into_iter().collect())
    }

    /// Negate `inner`.
    pub fn not(&self, inner: Predicate<E>) -> Predicate<E> {
        Predicate::Not(Box::new(inner))
    }

    /// Scope `E` by a condition on a related entity `R`, reached via `relation`
    /// (a variant of `E`'s SeaORM `Relation` enum). The closure builds the
    /// sub-condition using `R`'s own typed columns.
    ///
    /// ```
    /// # use nest_rs_authz::{AbilityBuilder, Action};
    /// # use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait};
    /// # mod conversation {
    /// #     use sea_orm::entity::prelude::*;
    /// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    /// #     #[sea_orm(table_name = "conversations")]
    /// #     pub struct Model {
    /// #         #[sea_orm(primary_key)]
    /// #         pub id: i32,
    /// #         pub org_id: i32,
    /// #     }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {}
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// # mod message {
    /// #     use sea_orm::entity::prelude::*;
    /// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    /// #     #[sea_orm(table_name = "messages")]
    /// #     pub struct Model {
    /// #         #[sea_orm(primary_key)]
    /// #         pub id: i32,
    /// #         pub conversation_id: i32,
    /// #     }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {
    /// #         #[sea_orm(
    /// #             belongs_to = "super::conversation::Entity",
    /// #             from = "Column::ConversationId",
    /// #             to = "super::conversation::Column::Id"
    /// #         )]
    /// #         Conversation,
    /// #     }
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// # fn main() -> Result<(), nest_rs_authz::MalformedRuleError> {
    /// # let org_id = 7;
    /// # let mut ab = AbilityBuilder::new();
    /// // `message` has no `org_id`; its tenant is the parent conversation's.
    /// ab.can(Action::Read, message::Entity).when(|p| {
    ///     p.related::<conversation::Entity, _>(
    ///         message::Relation::Conversation,
    ///         |c| c.eq(conversation::Column::OrgId, org_id),
    ///     )
    /// });
    /// # let ability = ab.build()?;
    ///
    /// let sql = message::Entity::find()
    ///     .filter(ability.condition_for::<message::Entity>(Action::Read))
    ///     .build(DatabaseBackend::Postgres)
    ///     .to_string();
    /// assert!(sql.ends_with(
    ///     r#"WHERE "messages"."conversation_id" IN (SELECT "conversations"."id" FROM "conversations" WHERE "conversations"."org_id" = 7)"#
    /// ));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A relation not pointing at `R`, or a composite key, logs an `error!` and
    /// returns [`Predicate::Deny`], which fails
    /// [`AbilityBuilder::build`](crate::AbilityBuilder::build).
    pub fn related<R, F>(&self, relation: E::Relation, build: F) -> Predicate<E>
    where
        R: EntityTrait,
        F: FnOnce(PredicateBuilder<R>) -> Predicate<R>,
    {
        let def = relation.def();
        let expected = R::default().table_ref();
        if def.to_tbl.sea_orm_table() != expected.sea_orm_table() {
            tracing::error!(
                target: crate::TARGET,
                related = std::any::type_name::<R>(),
                reason = "relation_table_mismatch",
                "invalid ability relation predicate — denying all rows",
            );
            return Predicate::Deny;
        }
        if def.from_col.arity() != 1 || def.to_col.arity() != 1 {
            tracing::error!(
                target: crate::TARGET,
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
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
            pub name: String,
            // Nullable column so `IsNull` and `Cmp`-on-NULL have something to bite on.
            pub tag: Option<String>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

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

    // `child` belongs_to `parent`; the tenant key lives only on the parent.
    mod parent {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "parents")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod child {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "children")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub parent_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {
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
        let p = b().lt(widget::Column::Tag, "m");
        assert!(!p.matches(&tagged(1, 7, "a", None)));
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
    fn a_relation_is_left_to_sql() {
        let p = PredicateBuilder::<child::Entity>::new()
            .related::<parent::Entity, _>(child::Relation::Parent, |c| {
                c.eq(parent::Column::OrgId, 7)
            });
        let row = child::Model {
            id: 1,
            parent_id: 99,
        };
        assert_eq!(p.decide(&row, &nothing_invented), Truth::Undecidable);
        assert!(!p.matches(&row), "a row shows nothing of its parent");
    }

    #[test]
    fn a_rule_on_an_invented_column_is_undecided() {
        let invented = |col: widget::Column| matches!(col, widget::Column::Tag);
        let row = tagged(1, 7, "a", Some("x"));
        for p in [
            b().eq(widget::Column::Tag, "x"),
            b().is_in(widget::Column::Tag, ["x"]),
            b().ne(widget::Column::Tag, "y"),
            b().gte(widget::Column::Tag, "a"),
            b().is_not_null(widget::Column::Tag),
        ] {
            assert_eq!(p.decide(&row, &invented), Truth::Undecidable);
            assert_eq!(
                p.decide(&row, &nothing_invented),
                Truth::True,
                "read, it is decided"
            );
        }
        assert_eq!(
            b().eq(widget::Column::OrgId, 7).decide(&row, &invented),
            Truth::True,
            "a column the row was not given stays decided",
        );
    }

    #[test]
    fn an_undecided_part_decides_only_what_the_others_leave_open() {
        let undecided = || PredicateBuilder::<widget::Entity>::new().is_null(widget::Column::Tag);
        let invented = |col: widget::Column| matches!(col, widget::Column::Tag);
        let row = model(1, 7, "a");
        let decide = |p: Predicate<widget::Entity>| p.decide(&row, &invented);

        assert_eq!(decide(undecided()), Truth::Undecidable);
        assert_eq!(
            decide(b().all([b().eq(widget::Column::OrgId, 8), undecided()])),
            Truth::False,
        );
        assert_eq!(
            decide(b().all([b().eq(widget::Column::OrgId, 7), undecided()])),
            Truth::Undecidable,
        );
        assert_eq!(
            decide(b().any([b().eq(widget::Column::OrgId, 7), undecided()])),
            Truth::True,
        );
        assert_eq!(
            decide(b().any([b().eq(widget::Column::OrgId, 8), undecided()])),
            Truth::Undecidable,
        );
        assert_eq!(decide(b().not(undecided())), Truth::Undecidable);
    }

    /// SQL's `tag <> 'x'` and `NOT (tag < 'm')` are `UNKNOWN` on a `NULL` tag,
    /// and its filter admits neither row.
    #[test]
    fn a_comparison_with_null_is_unknown_and_so_is_its_negation() {
        let row = tagged(1, 7, "a", None);
        for p in [
            b().ne(widget::Column::Tag, "x"),
            b().not(b().lt(widget::Column::Tag, "m")),
            b().not(b().eq(widget::Column::Tag, "x")),
        ] {
            assert_eq!(p.decide(&row, &nothing_invented), Truth::Unknown);
            assert!(!p.matches(&row));
        }
        assert_eq!(
            b().any([
                b().ne(widget::Column::Tag, "x"),
                b().eq(widget::Column::OrgId, 7)
            ])
            .decide(&row, &nothing_invented),
            Truth::True,
        );
    }

    /// sea-orm renders `eq(col, None)` as `IS NULL` and `ne(col, None)` as
    /// `IS NOT NULL`, which the row decides, negated or not.
    #[test]
    fn an_eq_or_ne_against_null_is_a_nullness_test() {
        let eq = || b().eq(widget::Column::Tag, None::<String>);
        let ne = || b().ne(widget::Column::Tag, None::<String>);
        assert!(sql(&eq()).contains(r#""tag" IS NULL"#), "{}", sql(&eq()));
        assert!(
            sql(&ne()).contains(r#""tag" IS NOT NULL"#),
            "{}",
            sql(&ne())
        );

        let untagged = tagged(1, 7, "a", None);
        let labelled = tagged(2, 7, "a", Some("x"));
        let decide =
            |p: Predicate<widget::Entity>, row: &widget::Model| p.decide(row, &nothing_invented);
        assert_eq!(decide(eq(), &untagged), Truth::True);
        assert_eq!(decide(eq(), &labelled), Truth::False);
        assert_eq!(decide(ne(), &untagged), Truth::False);
        assert_eq!(decide(ne(), &labelled), Truth::True);
        assert_eq!(decide(b().not(eq()), &labelled), Truth::True);
        assert_eq!(decide(b().not(ne()), &untagged), Truth::True);
        assert!(eq().matches(&untagged) && ne().matches(&labelled));
    }

    /// Only `eq` and `ne` turn a `NULL` value into a nullness test; an ordering
    /// against one compares with `NULL`.
    #[test]
    fn an_ordering_against_null_stays_unknown() {
        let row = tagged(1, 7, "a", Some("x"));
        assert!(sql(&b().lt(widget::Column::Tag, None::<String>)).contains(r#""tag" < NULL"#));
        for p in [
            b().lt(widget::Column::Tag, None::<String>),
            b().not(b().gte(widget::Column::Tag, None::<String>)),
        ] {
            assert_eq!(p.decide(&row, &nothing_invented), Truth::Unknown);
        }
    }

    /// A grant read by the instance check takes its relation to hold, so a
    /// negated relation grants nothing on a row that cannot show it.
    #[test]
    fn holding_relations_matches_a_relation_and_refuses_its_negation() {
        let p = || PredicateBuilder::<child::Entity>::new();
        let in_org = || {
            p().related::<parent::Entity, _>(child::Relation::Parent, |c| {
                c.eq(parent::Column::OrgId, 7)
            })
        };
        let row = child::Model {
            id: 1,
            parent_id: 99,
        };
        assert_eq!(in_org().decide_holding_relations(&row), Truth::True);
        assert_eq!(
            p().not(in_org()).decide_holding_relations(&row),
            Truth::False
        );
        assert_eq!(
            p().all([p().eq(child::Column::Id, 2), in_org()])
                .decide_holding_relations(&row),
            Truth::False,
            "the row's own columns still decide what they can",
        );
        assert_eq!(in_org().decide(&row, &nothing_invented), Truth::Undecidable);
    }

    #[test]
    fn a_list_holding_null_misses_as_unknown() {
        let p = b().is_in(widget::Column::Tag, [Some("a"), None]);
        assert_eq!(
            p.decide(&tagged(1, 7, "a", Some("a")), &nothing_invented),
            Truth::True
        );
        assert_eq!(
            p.decide(&tagged(1, 7, "a", Some("b")), &nothing_invented),
            Truth::Unknown
        );
        assert_eq!(
            b().not(p)
                .decide(&tagged(1, 7, "a", Some("b")), &nothing_invented),
            Truth::Unknown,
        );
    }

    #[test]
    fn two_value_types_only_the_database_compares_are_undecided() {
        let row = model(1, 7, "a");
        assert_eq!(
            b().eq(widget::Column::OrgId, 7_i64)
                .decide(&row, &nothing_invented),
            Truth::Undecidable,
        );
        assert_eq!(
            b().not(b().eq(widget::Column::OrgId, 8_i64))
                .decide(&row, &nothing_invented),
            Truth::Undecidable,
            "negated, a guess would admit the row",
        );
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
        // The relation targets `parent`, but `R` is declared as `child`.
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
