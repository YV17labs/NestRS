use nest_rs_authz::__private::Uncontained;
use sea_orm::sea_query::{Condition, IntoCondition, NullOrdering, Order};
use sea_orm::{
    DbBackend, DbErr, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    QueryTrait, Select, SelectExt, Statement,
};

use crate::contained::{contained, contained_order, refused};
use crate::repo::Repo;

/// A query [`Repo`] built — [`scoped`](Repo::scoped), an escape such as
/// [`unscoped`](Repo::unscoped) — that runs on the ambient executor.
///
/// It narrows through its own methods, in sea-orm's vocabulary, then runs with
/// [`all`](Self::all), [`one`](Self::one), [`count`](Self::count) or
/// [`exists`](Self::exists), each on the unit's executor and failing closed
/// outside one.
///
/// ```no_run
/// # mod post {
/// #     use sea_orm::entity::prelude::*;
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #     #[sea_orm(table_name = "posts")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         pub id: i32,
/// #         pub title: String,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// use nest_rs_authz::Action;
/// use nest_rs_seaorm::Repo;
/// use sea_orm::ColumnTrait;
///
/// # async fn titled(title: &str) -> Result<(), sea_orm::DbErr> {
/// let posts = Repo::<post::Entity>::scoped(Action::Read)
///     .filter(post::Column::Title.eq(title))
///     .order_by_asc(post::Column::Id)
///     .all()
///     .await?;
/// # Ok(())
/// # }
/// ```
///
/// It implements none of sea-orm's builder traits, so it hands out no statement:
/// nothing replaces, clears or `union`s what `Repo` filtered, or moves a hidden
/// column into an exposed one's slot.
///
/// ```compile_fail,E0599
/// # use nest_rs_authz::Action;
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::{EntityTrait, QueryTrait};
/// # fn rewritten<E: EntityTrait>() {
/// let mut select = Repo::<E>::scoped(Action::Read);
/// *select.query() = E::find().into_query();
/// # }
/// ```
///
/// ```compile_fail,E0599
/// # use nest_rs_authz::Action;
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::{EntityTrait, QueryFilter};
/// # fn through_filter<E: EntityTrait>() {
/// let mut select = Repo::<E>::scoped(Action::Read);
/// let statement = select.query();
/// # }
/// ```
///
/// ```compile_fail,E0599
/// # use nest_rs_authz::Action;
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::{EntityTrait, QueryOrder};
/// # fn through_order<E: EntityTrait>() {
/// let mut select = Repo::<E>::scoped(Action::Read);
/// let statement = select.query();
/// # }
/// ```
///
/// ```compile_fail,E0599
/// # use nest_rs_authz::Action;
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::{EntityTrait, QueryTrait};
/// # fn taken<E: EntityTrait>() {
/// let statement = Repo::<E>::scoped(Action::Read).into_query();
/// # }
/// ```
///
/// ```compile_fail,E0599
/// # use nest_rs_authz::Action;
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::{EntityTrait, QuerySelect};
/// # fn projected<E: EntityTrait>() {
/// let bare = Repo::<E>::scoped(Action::Read).select_only();
/// # }
/// ```
///
/// ```compile_fail,E0599
/// # use nest_rs_authz::Action;
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::{EntityTrait, QuerySelect};
/// # fn moved<E: EntityTrait>(hidden: E::Column) {
/// let swapped = Repo::<E>::scoped(Action::Read).column_as(hidden, "email");
/// # }
/// ```
///
/// An order key is a column of the entity, never an expression, so an order
/// computes nothing over a hidden column; an `Order::Field`, which writes its values
/// into the statement, refuses one whose enum type name would leave its quotes.
///
/// ```compile_fail,E0308
/// # use nest_rs_authz::Action;
/// # use nest_rs_seaorm::Repo;
/// # use sea_orm::EntityTrait;
/// # use sea_orm::sea_query::Expr;
/// # fn computed<E: EntityTrait>() {
/// let ordered = Repo::<E>::scoped(Action::Read).order_by_asc(Expr::val(1));
/// # }
/// ```
#[derive(Clone, Debug)]
#[must_use = "a select runs nothing until `all`, `one`, `count` or `exists`"]
pub struct RepoSelect<E: EntityTrait> {
    select: Select<E>,
    refused: Option<Uncontained>,
}

impl<E: EntityTrait> RepoSelect<E> {
    /// `select`, as `Repo` filtered it.
    pub(crate) fn new(select: Select<E>) -> Self {
        Self {
            select,
            refused: None,
        }
    }

    /// The query, its select as `step` leaves it.
    fn map_select(self, step: impl FnOnce(Select<E>) -> Select<E>) -> Self {
        Self {
            select: step(self.select),
            ..self
        }
    }

    /// Narrows the query by a condition the framework built — a scope, a key —
    /// which holds no text of the developer's.
    pub(crate) fn within(self, condition: impl IntoCondition) -> Self {
        self.map_select(|select| select.filter(condition))
    }

    /// The query, refused for `why` when there is one: it then builds `FALSE` and runs
    /// nothing.
    fn refusing(self, why: Option<Uncontained>) -> Self {
        if why.is_none() {
            return self;
        }
        Self {
            select: self.select.filter(Condition::any()),
            refused: self.refused.or(why),
        }
    }

    /// Narrows the query by `condition`, conjoined with what it holds as one
    /// parenthesized operand, so nothing it holds reaches past what `Repo` filtered.
    ///
    /// A raw fragment (`Expr::cust`, and every call writing text unescaped) keeps
    /// working when its text closes every parenthesis, string and identifier it
    /// opens and holds no comment, as a function call or a full-text search does;
    /// one that does not is refused before any statement, the query then running
    /// nothing and answering an opaque error, said at `warn` by its entity and never
    /// by its text.
    ///
    /// ```no_run
    /// # mod post {
    /// #     use sea_orm::entity::prelude::*;
    /// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    /// #     #[sea_orm(table_name = "posts")]
    /// #     pub struct Model {
    /// #         #[sea_orm(primary_key)]
    /// #         pub id: i32,
    /// #         pub body: String,
    /// #     }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {}
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// use nest_rs_authz::Action;
    /// use nest_rs_seaorm::Repo;
    /// use sea_orm::sea_query::Expr;
    ///
    /// # async fn search(words: &str) -> Result<(), sea_orm::DbErr> {
    /// #[expect(clippy::disallowed_methods, reason = "full-text search, its words bound")]
    /// let matching = Expr::cust_with_values(
    ///     r#"to_tsvector('english', "posts"."body") @@ plainto_tsquery('english', $1)"#,
    ///     [words],
    /// );
    /// let posts = Repo::<post::Entity>::scoped(Action::Read)
    ///     .filter(matching)
    ///     .all()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn filter(self, condition: impl IntoCondition) -> Self {
        match contained(condition.into_condition()) {
            Ok(None) => self,
            Ok(Some(operand)) => self.within(operand),
            Err(why) => self.refusing(Some(why)),
        }
    }

    /// Orders the rows by `column`, `order`; each order breaks the ties of the
    /// one before it.
    pub fn order_by(self, column: E::Column, order: Order) -> Self {
        let refusal = contained_order(&order).err();
        self.map_select(|select| select.order_by(column, order))
            .refusing(refusal)
    }

    /// Orders the rows by `column`, ascending.
    pub fn order_by_asc(self, column: E::Column) -> Self {
        self.map_select(|select| select.order_by_asc(column))
    }

    /// Orders the rows by `column`, descending.
    pub fn order_by_desc(self, column: E::Column) -> Self {
        self.map_select(|select| select.order_by_desc(column))
    }

    /// Orders the rows by `column`, `order`, with its `NULL`s where `nulls` says.
    pub fn order_by_with_nulls(self, column: E::Column, order: Order, nulls: NullOrdering) -> Self {
        let refusal = contained_order(&order).err();
        self.map_select(|select| select.order_by_with_nulls(column, order, nulls))
            .refusing(refusal)
    }

    /// Keeps at most `limit` rows; `None` keeps them all.
    pub fn limit(self, limit: impl Into<Option<u64>>) -> Self {
        self.map_select(|select| select.limit(limit))
    }

    /// Skips the first `offset` rows; `None` skips none.
    pub fn offset(self, offset: impl Into<Option<u64>>) -> Self {
        self.map_select(|select| select.offset(offset))
    }

    /// Applies `narrow` with the value `value` holds, the query as it is on `None`.
    pub fn apply_if<T>(self, value: Option<T>, narrow: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => narrow(self, value),
            None => self,
        }
    }

    /// The statement the query runs on `backend`, to read: nothing here runs one. A
    /// query [`filter`](Self::filter) refused builds `FALSE` in that condition's place.
    pub fn build(&self, backend: DbBackend) -> Statement {
        self.select.build(backend)
    }

    /// The select to run, or the refusal [`filter`](Self::filter) met.
    fn runnable(self) -> Result<Select<E>, DbErr> {
        match self.refused {
            Some(why) => Err(refused::<E>(why)),
            None => Ok(self.select),
        }
    }

    /// Every row the query matches.
    pub async fn all(self) -> Result<Vec<E::Model>, DbErr> {
        self.runnable()?.all(&Repo::<E>::conn()?).await
    }

    /// The first row the query matches, or `None`.
    pub async fn one(self) -> Result<Option<E::Model>, DbErr> {
        self.runnable()?.one(&Repo::<E>::conn()?).await
    }

    /// How many rows the query matches.
    pub async fn count(self) -> Result<u64, DbErr>
    where
        E::Model: Sync,
    {
        self.runnable()?.count(&Repo::<E>::conn()?).await
    }

    /// Whether the query matches any row.
    pub async fn exists(self) -> Result<bool, DbErr> {
        self.runnable()?.exists(&Repo::<E>::conn()?).await
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use nest_rs_authz::Action;
    use sea_orm::sea_query::{
        ArrayType, BinOper, Condition, Expr, ExprTrait, Func, Iden, IntoIden, Keyword,
        NullOrdering, OnConflict, OptionEnum, Order, OrderedStatement, OverStatement, PgFunc,
        Query, SelectStatement, SubQueryStatement, Value, Values, WindowStatement,
    };
    use sea_orm::{ColumnTrait, DbBackend, DbErr, EntityTrait, QueryFilter, QueryTrait};

    use super::RepoSelect;
    use crate::repo::Repo;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub rank: Option<i32>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// A derive's expansion sits beside the item carrying it, where an `#[expect]` on
    /// that item does not reach, so the opening for what it expands to goes on the module.
    #[expect(
        clippy::disallowed_methods,
        reason = "`select_as` and `save_as` expand to `cast_as`, and these casts name a constant"
    )]
    mod cast_column {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "tags")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            #[sea_orm(select_as = "text", save_as = "citext")]
            pub label: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    #[expect(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "with no `rs_type` the derive expands to `sea_query::Enum`, naming a constant"
    )]
    mod native_enum {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
        #[sea_orm(db_type = "Enum", enum_name = "tea")]
        pub(super) enum Tea {
            #[sea_orm(string_value = "green")]
            Green,
        }
    }

    /// With `rs_type = "String"` the derive expands to nothing the list names.
    mod native_enum_as_a_string {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
        #[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "tea")]
        pub(super) enum Tea {
            #[sea_orm(string_value = "green")]
            Green,
        }
    }

    use widget::Column;

    type Widgets = Repo<widget::Entity>;

    const SELECT: &str = r#"SELECT "widgets"."id", "widgets"."rank" FROM "widgets""#;

    fn sql(select: RepoSelect<widget::Entity>) -> String {
        select.build(DbBackend::Postgres).to_string()
    }

    /// What each way of running a query answered.
    async fn every_run() -> [Result<(), DbErr>; 4] {
        [
            Widgets::scoped(Action::Read).all().await.map(drop),
            Widgets::unscoped().one().await.map(drop),
            Widgets::with_tombstones(Action::Read)
                .count()
                .await
                .map(drop),
            Widgets::unscoped_by_id(7).exists().await.map(drop),
        ]
    }

    #[tokio::test]
    async fn a_query_outside_a_unit_fails_closed_for_want_of_an_executor() {
        for answer in every_run().await {
            assert!(
                matches!(&answer, Err(err) if err.to_string().contains("no ambient database executor")),
                "{answer:?}",
            );
        }
    }

    #[test]
    fn a_filter_conjoins_with_what_repo_filtered_as_one_operand() {
        assert_eq!(
            sql(Widgets::scoped(Action::Read).filter(Column::Id.eq(7))),
            format!(r#"{SELECT} WHERE (1 = 0) AND ("widgets"."id" = 7)"#),
        );
        assert_eq!(
            sql(Widgets::unscoped()
                .filter(Column::Id.eq(7))
                .filter(Column::Rank.is_null())),
            format!(r#"{SELECT} WHERE ("widgets"."id" = 7) AND ("widgets"."rank" IS NULL)"#),
        );
        assert_eq!(
            sql(Widgets::unscoped().filter(Condition::all())),
            SELECT,
            "an empty `all` is TRUE, and adds nothing",
        );
        assert_eq!(
            sql(Widgets::unscoped().filter(Condition::any())),
            format!("{SELECT} WHERE (FALSE)"),
            "an empty `any` is FALSE, and stays",
        );
    }

    #[test]
    fn a_condition_any_stays_inside_the_filter_it_joins() {
        let either = Condition::any().add(Column::Id.eq(7)).add(Column::Id.eq(8));
        assert_eq!(
            sql(Widgets::scoped(Action::Read).filter(either)),
            format!(r#"{SELECT} WHERE (1 = 0) AND ("widgets"."id" = 7 OR "widgets"."id" = 8)"#),
        );
    }

    /// Text closing what holds it, a parenthesis at any depth or a comment reading the
    /// rest of the statement as its own, is refused: the query builds `FALSE` in its
    /// place and runs nothing, before it needs an executor.
    #[tokio::test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the fragments under test: what the scan exists to refuse"
    )]
    async fn a_raw_fragment_closing_its_operand_is_refused_before_any_statement() {
        for fragment in [
            "1 = 1) OR (1 = 1",
            "1 = 1)) OR ((1 = 1",
            "1 = 1 OR true --",
            "1 = 1 /* ) OR ( */",
            "'a' = 'a",
            "1 = $$) OR ($$",
        ] {
            let refused = Widgets::scoped(Action::Read).filter(Expr::cust(fragment));
            assert_eq!(
                sql(refused.clone()),
                format!("{SELECT} WHERE (1 = 0) AND FALSE"),
                "{fragment}",
            );
            let answer = refused.all().await;
            assert!(
                matches!(&answer, Err(DbErr::Custom(said))
                    if said.contains("refused before any statement") && !said.contains(fragment)),
                "{fragment}: {answer:?}",
            );
        }
    }

    #[tokio::test]
    #[expect(clippy::disallowed_methods, reason = "the fragment under test")]
    async fn a_refusal_is_said_at_warn_by_entity_and_reason_never_by_its_text() {
        let logs = nest_rs_testing::LogCapture::install();
        let fragment = "1 = 1) OR (1 = 1";
        let answer = Widgets::unscoped()
            .filter(Expr::cust(fragment))
            .count()
            .await;

        assert!(answer.is_err(), "{answer:?}");
        let event = logs.expect_one(
            crate::target::ORM,
            "a condition or value handed to Repo refused — its text would not stay where Repo puts it",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("entity").as_deref(), Some("widgets"));
        assert_eq!(
            event.field("reason").as_deref(),
            Some("unbalanced_parenthesis")
        );
        assert!(!format!("{event:?}").contains(fragment), "{event:#?}");
    }

    /// What reads past a column without closing anything: an `OR` the text holds, which
    /// precedence would read past the scope beside it, stays inside the operand.
    #[test]
    #[expect(clippy::disallowed_methods, reason = "the fragment under test")]
    fn a_balanced_fragment_holding_an_or_stays_one_operand_beside_the_scope() {
        assert_eq!(
            sql(Widgets::scoped(Action::Read).filter(Expr::cust("a = 1 OR b = 2"))),
            format!("{SELECT} WHERE (1 = 0) AND (a = 1 OR b = 2)"),
        );
        let reaching = Column::Id.eq(7).or(Expr::cust("1 = 1"));
        assert_eq!(
            sql(Widgets::scoped(Action::Read).filter(reaching)),
            format!(r#"{SELECT} WHERE (1 = 0) AND ("widgets"."id" = 7 OR (1 = 1))"#),
        );
    }

    /// What a developer writes when no typed call reaches it keeps working, with
    /// nothing new to write: a function call, a full-text search, a string holding
    /// what the scan would refuse outside it.
    #[test]
    #[expect(clippy::disallowed_methods, reason = "the fragments under test")]
    fn a_fragment_closing_all_it_opens_keeps_working() {
        let searched = Expr::cust_with_values(
            r#"to_tsvector('english', "widgets"."name") @@ plainto_tsquery('english', $1)"#,
            ["tea -- or ) coffee"],
        );
        assert_eq!(
            sql(Widgets::scoped(Action::Read).filter(searched)),
            format!(
                r#"{SELECT} WHERE (1 = 0) AND (to_tsvector('english', "widgets"."name") @@ plainto_tsquery('english', 'tea -- or ) coffee'))"#
            ),
        );
        let called = Expr::expr(Func::cust("width_bucket").args([
            Expr::col(Column::Rank),
            Expr::val(0),
            Expr::val(100),
            Expr::val(4),
        ]))
        .eq(2);
        assert_eq!(
            sql(Widgets::unscoped().filter(called)),
            format!(r#"{SELECT} WHERE (width_bucket("rank", 0, 100, 4) = 2)"#),
        );
        assert_eq!(
            sql(Widgets::unscoped().filter(Expr::cust("note = ') OR (' AND \"a) OR (b\" = 1"))),
            format!(r#"{SELECT} WHERE (note = ') OR (' AND "a) OR (b" = 1)"#),
        );
    }

    #[test]
    fn each_order_is_on_a_column_and_breaks_the_ties_of_the_last() {
        let ordered = Widgets::unscoped()
            .order_by(Column::Rank, Order::Desc)
            .order_by_asc(Column::Id)
            .order_by_desc(Column::Rank)
            .order_by_with_nulls(Column::Rank, Order::Asc, NullOrdering::Last);
        assert_eq!(
            sql(ordered),
            format!(
                r#"{SELECT} ORDER BY "widgets"."rank" DESC, "widgets"."id" ASC, "widgets"."rank" DESC, "widgets"."rank" ASC NULLS LAST"#
            ),
        );
    }

    #[test]
    fn a_limit_and_an_offset_bound_the_rows_and_none_lifts_each() {
        let bounded = Widgets::unscoped().limit(2).offset(3);
        assert_eq!(sql(bounded.clone()), format!("{SELECT} LIMIT 2 OFFSET 3"));
        assert_eq!(sql(bounded.limit(None).offset(None)), SELECT);
    }

    #[test]
    fn apply_if_narrows_on_a_value_and_leaves_the_query_on_none() {
        let by = |id: Option<i32>| {
            sql(Widgets::unscoped().apply_if(id, |select, id| select.filter(Column::Id.eq(id))))
        };
        assert_eq!(
            by(Some(7)),
            format!(r#"{SELECT} WHERE ("widgets"."id" = 7)"#)
        );
        assert_eq!(by(None), SELECT);
    }

    /// Why each entry of the `clippy.toml` list is on it, read off what it renders.
    /// Each carries its own `#[expect]`, so an entry leaving the list fails here.
    #[test]
    fn every_constructor_the_lint_names_splices_its_text() {
        // Both quotes, so neither a string's escaping nor an identifier's keeps it as is.
        const RAW: &str = r#"1 = 1') OR ("a" = 'a"#;
        macro_rules! each_opened {
            ($($fragment:expr),* $(,)?) => {
                [$(
                    #[expect(clippy::disallowed_methods, reason = "the entry under test")]
                    $fragment
                ),*]
            };
        }
        #[expect(clippy::disallowed_types, reason = "the entry under test")]
        use sea_orm::sea_query::Enum;
        let named = || Enum {
            type_name: RAW.into(),
            value: "a".into(),
        };
        let subquery = |select: &mut SelectStatement| {
            Expr::col(Column::Id).in_subquery(select.column(Column::Id).from(widget::Entity).take())
        };
        let windowed =
            |window: WindowStatement| subquery(Query::select().expr_window(Expr::val(1), window));
        let update = || Query::update().table(widget::Entity).take();
        let delete = || Query::delete().from_table(widget::Entity).take();
        let splicing = each_opened![
            Expr::cust(RAW),
            Expr::cust_with_values(RAW, [0; 0]),
            Expr::cust_with_expr(RAW, Expr::val(1)),
            Expr::cust_with_exprs(RAW, []),
            Expr::Custom(RAW.into()),
            Expr::CustomWithExpr(RAW.into(), vec![]),
            Expr::custom_keyword(RAW),
            Expr::Keyword(Keyword::Custom(RAW.into_iden())),
            Func::cust(RAW).into(),
            Func::cast_as(1, RAW).into(),
            ExprTrait::cast_as(Expr::val(1), RAW),
            ExprTrait::as_enum(Expr::val(1), RAW),
            Expr::AsEnum(RAW.into_iden(), Box::new(Expr::val(1))),
            Expr::Constant(named().into()),
            Expr::val(Value::Enum(OptionEnum::None(RAW.into()))),
            Expr::val(Value::Array(ArrayType::Enum(Box::new(RAW.into())), None)),
            Expr::val(1).binary(BinOper::Custom(RAW), Expr::val(1)),
            PgFunc::json_exists(Expr::val("{}"), RAW).into(),
            PgFunc::json_value(Expr::val("{}"), RAW).into(),
            PgFunc::json_query(Expr::val("{}"), RAW).into(),
            PgFunc::json_table(Expr::val("{}"), RAW).into(),
            subquery(
                Query::select().order_by(Column::Id, Order::Field(Values(vec![named().into()])))
            ),
            subquery(Query::select().order_by_customs([(RAW, Order::Asc)])),
            subquery(Query::select().order_by_customs_with_nulls([(
                RAW,
                Order::Asc,
                NullOrdering::Last,
            )])),
            Expr::from(SubQueryStatement::from(
                update().order_by_customs([(RAW, Order::Asc)]).take(),
            )),
            Expr::from(SubQueryStatement::from(
                update()
                    .order_by_customs_with_nulls([(RAW, Order::Asc, NullOrdering::Last)])
                    .take(),
            )),
            Expr::from(SubQueryStatement::from(
                delete().order_by_customs([(RAW, Order::Asc)]).take(),
            )),
            Expr::from(SubQueryStatement::from(
                delete()
                    .order_by_customs_with_nulls([(RAW, Order::Asc, NullOrdering::Last)])
                    .take(),
            )),
            Expr::from(SubQueryStatement::from(
                Query::insert()
                    .into_table(widget::Entity)
                    .on_conflict(OnConflict::constraint(RAW))
                    .take(),
            )),
            windowed(
                WindowStatement::new()
                    .order_by_customs([(RAW, Order::Asc)])
                    .take(),
            ),
            windowed(
                WindowStatement::new()
                    .order_by_customs_with_nulls([(RAW, Order::Asc, NullOrdering::Last)])
                    .take(),
            ),
            windowed(WindowStatement::partition_by_custom(RAW)),
            windowed(
                OverStatement::partition_by_customs(&mut WindowStatement::new(), [RAW]).take(),
            ),
            subquery(OrderedStatement::order_by_customs(
                &mut Query::select(),
                [(RAW, Order::Asc)],
            )),
            subquery(OrderedStatement::order_by_customs_with_nulls(
                &mut Query::select(),
                [(RAW, Order::Asc, NullOrdering::Last)],
            )),
        ];
        let mut fragments = Vec::from(splicing);
        fragments.push(Expr::val(named()));
        for fragment in fragments {
            let rendered = widget::Entity::find()
                .filter(fragment)
                .build(DbBackend::Postgres)
                .to_string();
            assert!(rendered.contains(RAW), "{rendered}");
        }
    }

    /// What the list cannot name: a literal is no call, so one through its full path
    /// passes clippy, and so does a hand-written `Iden` borrowing its text. The text
    /// each writes stays inside the operand, or leaves it and is refused.
    #[tokio::test]
    async fn an_enum_literal_through_its_full_path_stays_inside_its_operand() {
        let named = |type_name: &str| sea_orm::sea_query::Enum {
            type_name: type_name.into(),
            value: "a".into(),
        };

        assert_eq!(
            sql(Widgets::scoped(Action::Read)
                .filter(Column::Id.eq(named(r#"text" = 'a' OR 1 = 1 OR 'a'::"text"#)))),
            format!(
                r#"{SELECT} WHERE (1 = 0) AND ("widgets"."id" = 'a'::"text" = 'a' OR 1 = 1 OR 'a'::"text")"#
            ),
        );

        let commenting =
            Widgets::scoped(Action::Read).filter(Column::Id.eq(named(r#"text" OR 1 = 1 --"#)));
        assert_eq!(
            sql(commenting.clone()),
            format!("{SELECT} WHERE (1 = 0) AND FALSE")
        );
        assert!(commenting.one().await.is_err());

        let ordered = Widgets::unscoped().order_by(
            Column::Id,
            #[expect(clippy::disallowed_methods, reason = "the order under test")]
            Order::Field(Values(vec![named(r#"text" OR 1 = 1 --"#).into()])),
        );
        assert_eq!(
            sql(ordered.clone()),
            format!(
                r#"{SELECT} WHERE FALSE ORDER BY CASE WHEN "widgets"."id"='a'::"text" OR 1 = 1 --" THEN 0 ELSE 1 END"#
            ),
            "an order by field writes its values inline, and one leaving its quotes is refused",
        );
        assert!(ordered.all().await.is_err());
    }

    #[test]
    fn a_hand_written_iden_borrowing_its_text_stays_inside_its_operand() {
        struct Widening;

        impl Iden for Widening {
            fn quoted(&self) -> Cow<'static, str> {
                Cow::Borrowed(r#"id" OR 1 = 1 OR "id"#)
            }

            fn unquoted(&self) -> &str {
                "id"
            }
        }

        assert_eq!(
            sql(Widgets::scoped(Action::Read).filter(Expr::col(Widening).is_not_null())),
            format!(r#"{SELECT} WHERE (1 = 0) AND ("id" OR 1 = 1 OR "id" IS NOT NULL)"#),
        );
    }

    #[test]
    fn a_derive_expanding_to_a_listed_call_renders_under_the_opening_on_its_module() {
        assert!(
            Repo::<cast_column::Entity>::unscoped()
                .build(DbBackend::Postgres)
                .to_string()
                .contains(r#"CAST("tags"."label" AS text)"#),
        );
        assert_eq!(
            sea_orm::ActiveEnum::to_value(&native_enum::Tea::Green).value,
            "green",
        );
        assert_eq!(
            sea_orm::ActiveEnum::to_value(&native_enum_as_a_string::Tea::Green),
            "green",
        );
    }
}
