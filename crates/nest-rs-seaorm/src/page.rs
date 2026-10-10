use std::collections::HashMap;
use std::hash::Hash;

use sea_orm::prelude::Uuid;
use sea_orm::sea_query::{
    Asterisk, Condition, Expr, ExprTrait, Order, Query, Value, ValueType, WindowStatement,
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, IdenStatic, Iterable, ModelTrait,
    PrimaryKeyToColumn, PrimaryKeyTrait, QueryFilter, QueryTrait,
};

use nest_rs_authz::Action;

use crate::repo::{Repo, scope_for};

/// One keyset page. `next_cursor` is the last row's primary key, present only
/// when [`has_more`](Page::has_more).
#[derive(Clone, Debug)]
pub struct Page<M> {
    /// The rows on this page, ascending by primary key.
    pub items: Vec<M>,
    /// Cursor to pass as `after` for the next page — the last row's key, set
    /// only when [`has_more`](Self::has_more).
    pub next_cursor: Option<Uuid>,
    /// Whether a further page exists (an extra row was over-fetched).
    pub has_more: bool,
}

/// Clamp the requested page size to the `1..=100` window [`PageParams::limit`]
/// applies.
pub fn clamp_page_size(first: u64) -> u64 {
    first.clamp(1, 100)
}

/// The page size a caller who asked for none gets — on the `?first=` query, on
/// a `#[crud]` list operation, and on an auto-resolved relation.
pub const DEFAULT_PAGE_SIZE: u64 = 20;

/// Hard backstop on `CrudService::list`: no unpaginated read returns more
/// rows than this — a capped result logs a `warn` naming the entity. A
/// collection that can grow past it must paginate (`CrudService::page`).
pub const LIST_CAP: u64 = 1_000;

/// Prefixed so neither alias can collide with a column of the paged entity.
const RANK_ALIAS: &str = "__nest_rs_rank";
const RANKED_ALIAS: &str = "__nest_rs_ranked";

/// Wrap an already-scoped child select so each parent keeps only its own first
/// `limit + 1` rows: rank within the partition, then filter on the rank.
fn rank_per_parent<E: EntityTrait>(
    scoped: sea_orm::Select<E>,
    fk: E::Column,
    pk: E::Column,
    limit: u64,
    after: Option<Uuid>,
) -> sea_orm::sea_query::SelectStatement {
    let scoped = match after {
        Some(after) => scoped.filter(pk.gt(after)),
        None => scoped,
    };
    let mut ranked = scoped.into_query();
    ranked.expr_window_as(
        Expr::cust("ROW_NUMBER()"),
        WindowStatement::partition_by(fk)
            .order_by(pk, Order::Asc)
            .take(),
        RANK_ALIAS,
    );

    let mut windowed = Query::select();
    windowed
        .column(Asterisk)
        .from_subquery(ranked, RANKED_ALIAS)
        .and_where(Expr::col(RANK_ALIAS).lte(limit + 1))
        // The caller buckets rows in arrival order and reads the last one.
        .order_by(fk, Order::Asc)
        .order_by(pk, Order::Asc);
    windowed
}

/// `(items, has_more)` from a `limit + 1` cursor fetch, truncated to `limit`.
pub(crate) fn split_overfetched<M>(mut items: Vec<M>, limit: u64) -> (Vec<M>, bool) {
    let has_more = items.len() as u64 > limit;
    items.truncate(limit as usize);
    (items, has_more)
}

/// `next_cursor` from a finished page: the last row's primary key when there
/// is more to fetch, else `None`.
pub(crate) fn next_cursor_from<M>(
    items: &[M],
    has_more: bool,
    pk: impl FnMut(&M) -> Option<Uuid>,
) -> Option<Uuid> {
    if has_more {
        items.last().and_then(pk)
    } else {
        None
    }
}

/// The `?first=&after=` cursor query. An unparsable `after` is ignored — paging
/// from the start, never an error.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct PageParams {
    /// Requested page size; defaults to 20 and is clamped to `1..=100`.
    pub first: Option<u64>,
    /// Opaque cursor from a prior page's `next` link; unparsable ⇒ from start.
    pub after: Option<String>,
}

impl PageParams {
    /// Page size, defaulting to 20 and clamped to `1..=100`.
    pub fn limit(&self) -> u64 {
        clamp_page_size(self.first.unwrap_or(DEFAULT_PAGE_SIZE))
    }

    /// The `after` cursor parsed as a primary key, or `None` when absent or
    /// malformed — an unparsable cursor pages from the start rather than erroring.
    pub fn after_uuid(&self) -> Option<Uuid> {
        self.after.as_deref().and_then(|s| Uuid::parse_str(s).ok())
    }
}

impl<E: EntityTrait> Repo<E>
where
    E::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    E::Model: Send + Sync,
{
    /// A keyset page of readable rows, ascending by primary key, starting after
    /// `after`. Fetches one extra row to decide `has_more` and `next_cursor`.
    /// `extra` is ANDed onto the ability scope (e.g. `deleted_at IS NULL`).
    pub async fn page(
        first: u64,
        after: Option<Uuid>,
        extra: Condition,
    ) -> Result<Page<E::Model>, DbErr> {
        let conn = Self::conn()?;
        let limit = clamp_page_size(first);

        let pk_col = Self::keyset_column()?;

        let mut cursor = E::find()
            .filter(scope_for::<E>(Action::Read))
            .filter(extra)
            .cursor_by(pk_col);
        if let Some(after) = after {
            cursor.after(after);
        }
        cursor.first(limit + 1);

        let (items, has_more) = split_overfetched(cursor.all(&conn).await?, limit);

        let next_cursor = next_cursor_from(&items, has_more, |model| {
            <Uuid as ValueType>::try_from(ModelTrait::get(model, pk_col)).ok()
        });

        Ok(Page {
            items,
            next_cursor,
            has_more,
        })
    }

    /// The column keyset pagination pages by; a `DbErr` for an entity with no
    /// primary key, which SeaORM permits (views, raw tables).
    fn keyset_column() -> Result<E::Column, DbErr> {
        let Some(pk) = E::PrimaryKey::iter().next() else {
            let entity = std::any::type_name::<E>();
            tracing::error!(
                target: crate::target::ORM,
                entity,
                "entity has no primary-key column — keyset pagination requires one",
            );
            return Err(DbErr::Custom(format!(
                "entity `{entity}` has no primary-key column; keyset pagination requires one"
            )));
        };
        Ok(pk.into_column())
    }

    /// **One keyset page per parent**, for every key in `keys`, in a single
    /// round trip. The read-side primitive an auto-resolved `has_many` relation
    /// is built on; `extra` is ANDed onto the ability scope exactly as in
    /// [`page`](Self::page) (e.g. `deleted_at IS NULL`).
    ///
    /// Every key gets an entry, empty when it has no children. The limit is
    /// applied **per partition** — a single `LIMIT` would starve the parents
    /// sorting last:
    ///
    /// ```sql
    /// SELECT * FROM (
    ///   SELECT …, ROW_NUMBER() OVER (PARTITION BY fk ORDER BY pk) AS rank
    ///   FROM child
    ///   WHERE <ability scope> AND <extra> AND fk IN (…) AND pk > <after>
    /// ) ranked
    /// WHERE rank <= limit + 1
    /// ```
    ///
    /// `ROW_NUMBER() OVER` needs MySQL 8, MariaDB 10.2 or SQLite 3.25.
    pub async fn relation_pages<K>(
        fk: E::Column,
        keys: &[K],
        first: u64,
        after: Option<Uuid>,
        extra: Condition,
    ) -> Result<HashMap<K, Page<E::Model>>, DbErr>
    where
        K: Clone + Eq + Hash + Into<Value> + ValueType + Send + Sync,
    {
        let mut pages: HashMap<K, Page<E::Model>> = keys
            .iter()
            .map(|key| {
                (
                    key.clone(),
                    Page {
                        items: Vec::new(),
                        next_cursor: None,
                        has_more: false,
                    },
                )
            })
            .collect();
        if pages.is_empty() {
            return Ok(pages);
        }

        let conn = Self::conn()?;
        let limit = clamp_page_size(first);
        let pk_col = Self::keyset_column()?;

        let scoped = E::find()
            .filter(scope_for::<E>(Action::Read))
            .filter(extra)
            .filter(fk.is_in(keys.iter().cloned()));
        let windowed = rank_per_parent(scoped, fk, pk_col, limit, after);

        let statement = conn.get_database_backend().build(&windowed);
        let rows = E::find().from_raw_sql(statement).all(&conn).await?;

        for row in rows {
            let value = ModelTrait::get(&row, fk);
            #[expect(clippy::map_err_ignore, reason = "K::Error is any type, with no Display bound to carry")]
            let key = K::try_from(value).map_err(|_| {
                DbErr::Custom(format!(
                    "relation page: foreign key `{}` on `{}` did not read back as the batch key type",
                    fk.as_str(),
                    std::any::type_name::<E>(),
                ))
            })?;
            let Some(page) = pages.get_mut(&key) else {
                return Err(DbErr::Custom(format!(
                    "relation page: `{}` returned a row outside the requested key set",
                    std::any::type_name::<E>(),
                )));
            };
            page.items.push(row);
        }

        for page in pages.values_mut() {
            let (items, has_more) = split_overfetched(std::mem::take(&mut page.items), limit);
            page.next_cursor = next_cursor_from(&items, has_more, |model| {
                <Uuid as ValueType>::try_from(ModelTrait::get(model, pk_col)).ok()
            });
            page.items = items;
            page.has_more = has_more;
        }

        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod child {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "child")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
            pub parent_id: Uuid,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    // `DeriveEntityModel` cannot express a keyless entity, so it is written out.
    mod keyless {
        use sea_orm::entity::prelude::*;

        #[derive(Copy, Clone, Default, Debug, DeriveEntity)]
        pub(super) struct Entity;

        impl EntityName for Entity {
            fn table_name(&self) -> &'static str {
                "keyless"
            }
        }

        #[derive(Clone, Debug, PartialEq, DeriveModel, DeriveActiveModel)]
        pub(super) struct Model {
            pub label: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
        pub(super) enum Column {
            Label,
        }

        impl ColumnTrait for Column {
            type EntityName = Entity;

            fn def(&self) -> ColumnDef {
                match self {
                    Self::Label => ColumnType::String(StringLen::None).def(),
                }
            }
        }

        #[derive(Copy, Clone, Debug, EnumIter)]
        pub(super) enum PrimaryKey {}

        impl sea_orm::Iden for PrimaryKey {
            fn unquoted(&self) -> &str {
                match *self {}
            }
        }

        impl sea_orm::IdenStatic for PrimaryKey {
            fn as_str(&self) -> &'static str {
                match *self {}
            }
        }

        impl PrimaryKeyTrait for PrimaryKey {
            type ValueType = Uuid;

            fn auto_increment() -> bool {
                false
            }
        }

        impl PrimaryKeyToColumn for PrimaryKey {
            type Column = Column;

            fn into_column(self) -> Self::Column {
                match self {}
            }

            fn from_column(_: Self::Column) -> Option<Self> {
                None
            }
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    #[test]
    fn an_entity_with_no_primary_key_is_refused_by_name_rather_than_panicking() {
        let logs = nest_rs_testing::LogCapture::install();

        let err = Repo::<keyless::Entity>::keyset_column()
            .expect_err("an entity with no primary key cannot be paged by one");
        let message = err.to_string();
        assert!(
            message.contains("keyless") && message.contains("primary-key"),
            "the error names the entity and the missing column: {message}",
        );

        let event = logs.expect_one(
            crate::target::ORM,
            "entity has no primary-key column — keyset pagination requires one",
        );
        assert_eq!(event.level, "error");
        assert!(
            event.field("entity").is_some_and(|e| e.contains("keyless")),
            "the event names the entity, got {:?}",
            event.fields,
        );
    }

    #[test]
    fn an_entity_with_a_primary_key_pages_by_it() {
        assert_eq!(
            Repo::<child::Entity>::keyset_column()
                .expect("a keyed entity pages")
                .as_str(),
            child::Column::Id.as_str(),
        );
    }

    fn relation_sql(limit: u64, after: Option<Uuid>) -> String {
        use sea_orm::sea_query::PostgresQueryBuilder;

        rank_per_parent(
            <child::Entity as EntityTrait>::find(),
            child::Column::ParentId,
            child::Column::Id,
            limit,
            after,
        )
        .to_string(PostgresQueryBuilder)
    }

    #[test]
    fn relation_pages_ranks_within_each_parent() {
        let sql = relation_sql(2, None);
        assert!(
            sql.contains(r#"ROW_NUMBER() OVER ( PARTITION BY "parent_id" ORDER BY "id" ASC )"#),
            "the limit must be applied per parent, not to the result set: {sql}",
        );
    }

    #[test]
    fn relation_pages_keeps_one_row_beyond_the_page_to_decide_has_more() {
        assert!(
            relation_sql(2, None).contains(r#""__nest_rs_rank" <= 3"#),
            "{}",
            relation_sql(2, None),
        );
    }

    #[test]
    fn relation_pages_orders_parents_together_and_rows_by_key() {
        let sql = relation_sql(5, None);
        assert!(
            sql.contains(r#"ORDER BY "parent_id" ASC, "id" ASC"#),
            "buckets are filled in arrival order, so the SQL must group and sort them: {sql}",
        );
    }

    #[test]
    fn relation_pages_applies_the_cursor_inside_the_ranking() {
        // Applied outside, a parent's rank would count rows the caller already
        // has, and page 2 would come back short.
        let after = Uuid::now_v7();
        let sql = relation_sql(2, Some(after));
        let (inner, outer) = sql
            .split_once(r#") AS "__nest_rs_ranked""#)
            .expect("the ranked subquery is aliased");
        assert!(inner.contains(r#""id" > "#), "{sql}");
        assert!(!outer.contains(r#""id" > "#), "{sql}");
    }

    fn params(first: Option<u64>, after: Option<&str>) -> PageParams {
        PageParams {
            first,
            after: after.map(str::to_owned),
        }
    }

    #[test]
    fn limit_defaults_to_20() {
        assert_eq!(params(None, None).limit(), 20);
    }

    #[test]
    fn limit_clamps_zero_up_to_one() {
        assert_eq!(params(Some(0), None).limit(), 1);
    }

    #[test]
    fn limit_clamps_above_one_hundred() {
        assert_eq!(params(Some(1_000), None).limit(), 100);
    }

    #[test]
    fn limit_passes_through_in_range_values() {
        assert_eq!(params(Some(50), None).limit(), 50);
    }

    #[test]
    fn after_uuid_returns_none_for_garbage() {
        assert!(params(None, Some("not-a-uuid")).after_uuid().is_none());
        assert!(params(None, Some("")).after_uuid().is_none());
    }

    #[test]
    fn after_uuid_round_trips_a_v7() {
        let uuid = Uuid::now_v7();
        let parsed = params(None, Some(&uuid.to_string())).after_uuid();
        assert_eq!(parsed, Some(uuid));
    }

    #[test]
    fn clamp_page_size_matches_params_window() {
        assert_eq!(clamp_page_size(0), 1);
        assert_eq!(clamp_page_size(1), 1);
        assert_eq!(clamp_page_size(20), 20);
        assert_eq!(clamp_page_size(100), 100);
        assert_eq!(clamp_page_size(u64::MAX), 100);
    }

    #[test]
    fn split_overfetched_under_limit_has_no_more() {
        let (items, more) = split_overfetched(vec![1, 2, 3], 5);
        assert_eq!(items, vec![1, 2, 3]);
        assert!(!more);
    }

    #[test]
    fn split_overfetched_exactly_at_limit_has_no_more() {
        let (items, more) = split_overfetched(vec![1, 2, 3], 3);
        assert_eq!(items, vec![1, 2, 3]);
        assert!(!more);
    }

    #[test]
    fn split_overfetched_over_limit_drops_the_probe_row_and_flags_more() {
        let (items, more) = split_overfetched(vec![1, 2, 3, 4], 3);
        assert_eq!(items, vec![1, 2, 3], "the probe row is truncated");
        assert!(
            more,
            "an over-fetched row means there is at least one more page"
        );
    }

    #[test]
    fn split_overfetched_empty_is_a_terminal_empty_page() {
        let (items, more) = split_overfetched::<i32>(vec![], 10);
        assert!(items.is_empty());
        assert!(!more);
    }

    #[test]
    fn page_struct_fields_are_publicly_constructible() {
        let cursor = Uuid::now_v7();
        let page = Page {
            items: vec!["a", "b"],
            next_cursor: Some(cursor),
            has_more: true,
        };
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.next_cursor, Some(cursor));
        assert!(page.has_more);
    }

    #[test]
    fn next_cursor_from_returns_last_pk_when_more_to_fetch() {
        let cursor = Uuid::now_v7();
        let next = next_cursor_from(&[1, 2, 3], true, |_| Some(cursor));
        assert_eq!(next, Some(cursor));
    }

    #[test]
    fn next_cursor_from_returns_none_on_a_terminal_page() {
        let mut calls = 0;
        let next = next_cursor_from(&[1, 2, 3], false, |_| {
            calls += 1;
            Some(Uuid::now_v7())
        });
        assert_eq!(next, None);
        assert_eq!(calls, 0, "the pk closure must not run on a terminal page");
    }

    #[test]
    fn next_cursor_from_handles_a_pk_extractor_returning_none() {
        let next = next_cursor_from(&[1, 2, 3], true, |_| None);
        assert_eq!(next, None);
    }

    #[test]
    fn next_cursor_from_on_empty_with_has_more_is_none() {
        let next = next_cursor_from::<i32>(&[], true, |_| Some(Uuid::now_v7()));
        assert_eq!(next, None);
    }

    #[test]
    fn next_cursor_from_passes_the_last_item_to_the_pk_closure() {
        let cursor = Uuid::now_v7();
        let mut seen = None;
        let next = next_cursor_from(&[10, 20, 30], true, |m| {
            seen = Some(*m);
            Some(cursor)
        });
        assert_eq!(next, Some(cursor));
        assert_eq!(seen, Some(30), "the closure receives the LAST item");
    }

    #[test]
    fn page_params_derives_clone_and_debug() {
        let p = params(Some(10), Some("not-a-uuid"));
        let cloned = p.clone();
        assert_eq!(cloned.first, Some(10));
        assert_eq!(cloned.after.as_deref(), Some("not-a-uuid"));
        assert!(format!("{p:?}").contains("not-a-uuid"));
    }
}
