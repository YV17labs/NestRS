//! Keyset (cursor) pagination over the primary key.
//!
//! Keyset beats offset for a feed: O(1) on the index, stable under concurrent
//! inserts. With UUID-v7 keys (time-ordered), paging by the key is also
//! chronological with no extra sort column.

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
///
/// `Clone` because an auto-resolved relation's page is a dataloader value, and
/// async-graphql hands one batch result to every caller waiting on it.
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

/// Clamp the requested page size to the `1..=100` window — the same bound
/// [`PageParams::limit`] applies, kept here so callers passing a `u64` (e.g.
/// the GraphQL pagination input) reuse one source of truth.
pub fn clamp_page_size(first: u64) -> u64 {
    first.clamp(1, 100)
}

/// The page size a caller who asked for none gets — on the `?first=` query, on
/// a `#[crud]` list operation, and on an auto-resolved relation. One constant so
/// "how many rows does an unparameterised page return" has one answer whichever
/// surface asked.
pub const DEFAULT_PAGE_SIZE: u64 = 20;

/// Hard backstop on `CrudService::list`: no unpaginated read returns more
/// rows than this, ever — a capped result logs a `warn` naming the entity.
/// Deliberately far above `clamp_page_size`'s window: the cap is a safety
/// net for "small, finite collection" callers, not a page size. A collection
/// that can grow past it must paginate (`CrudService::page`).
pub const LIST_CAP: u64 = 1_000;

/// SQL alias for the per-parent rank a relation page ranks its rows by, and for
/// the subquery that carries it. Prefixed so neither can collide with a real
/// column of the entity being paged.
const RANK_ALIAS: &str = "__nest_rs_rank";
const RANKED_ALIAS: &str = "__nest_rs_ranked";

/// Wrap an already-scoped child select so each parent keeps only its own first
/// `limit + 1` rows: rank within the partition, then filter on the rank.
///
/// The query-shape half of [`Repo::relation_pages`], extracted so the SQL it
/// emits is assertable without a database — the rest of that method is bucketing
/// rows a real query returned, and a wrong `PARTITION BY` would still return
/// plausible-looking rows.
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
    // Unqualified column refs: the window sits over a single-table select, so
    // `fk`/`pk` are unambiguous — and qualifying them would have to reproduce
    // whatever table reference SeaORM chose.
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
        // Deterministic per parent: the caller buckets rows in arrival order,
        // and `has_more` / `next_cursor` read the last one.
        .order_by(fk, Order::Asc)
        .order_by(pk, Order::Asc);
    windowed
}

/// `(items, has_more)` from a `limit + 1` cursor fetch. Truncates `items` to
/// `limit` when an extra row was returned. The pure-data half of `Repo::page`,
/// extracted so its boundary behaviour is unit-testable without a DB.
pub fn split_overfetched<M>(mut items: Vec<M>, limit: u64) -> (Vec<M>, bool) {
    let has_more = items.len() as u64 > limit;
    items.truncate(limit as usize);
    (items, has_more)
}

/// `next_cursor` from a finished page: the last row's primary key when there
/// is more to fetch, else `None`. Splits a closure-heavy `if`-`else` out of
/// `Repo::page` so the cursor-selection branches are testable as pure logic.
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
    /// Opaque cursor from a prior page's `next_cursor`; unparsable ⇒ from start.
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

    /// The column keyset pagination pages by.
    ///
    /// SeaORM permits primary-key-less entities (views, raw tables), so this is
    /// a typed `DbErr` naming the entity rather than a panic on a query hot
    /// path — the layer's contract is "never panic, return `DbErr`".
    fn keyset_column() -> Result<E::Column, DbErr> {
        let Some(pk) = E::PrimaryKey::iter().next() else {
            let entity = std::any::type_name::<E>();
            tracing::error!(
                target: "nest_rs::orm",
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
    /// Every key gets an entry, so "no children" and "not asked for" stay
    /// distinguishable at the call site — an absent parent is a bug, an empty
    /// [`Page`] is an answer.
    ///
    /// # Why this is not `WHERE fk IN (keys) LIMIT n`
    ///
    /// It cannot be. A single `LIMIT` bounds the *result set*, not each
    /// parent's slice of it, so `cap × keys` rows ordered by the foreign key
    /// are consumed by whichever parents sort first and the rest read as `[]` —
    /// indistinguishable from having no children. That was a silent wrong
    /// answer, and no amount of over-fetching fixes it: the row a starved
    /// parent needs may be arbitrarily far down.
    ///
    /// So the limit is applied **per partition**, by ranking rows within each
    /// parent and keeping the first `limit + 1`:
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
    /// The extra row per parent is what decides
    /// [`has_more`](Page::has_more), the same over-fetch
    /// [`page`](Self::page) uses.
    ///
    /// `ROW_NUMBER() OVER (PARTITION BY …)` is SQL:2003 and is implemented by
    /// every backend SeaORM supports at a currently-maintained version
    /// (PostgreSQL — the backend this workspace builds against — MySQL 8,
    /// MariaDB 10.2, SQLite 3.25). The inner query is still built by
    /// [`scoped`](Repo::scoped), so the ability filter is applied by the same
    /// code path as every other read; only the ranking wrapper is hand-built.
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
            let key = K::try_from(value).map_err(|_| {
                DbErr::Custom(format!(
                    "relation page: foreign key `{}` on `{}` did not read back as the batch key type",
                    fk.as_str(),
                    std::any::type_name::<E>(),
                ))
            })?;
            // A row whose key is not in the batch cannot happen (the `IN` list
            // is the batch), but dropping it is still wrong to do silently.
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

    // A minimal child entity, so the relation-page query shape can be asserted
    // as SQL text. The bug this guards is invisible in a row count: a `LIMIT`
    // on the result set instead of a rank per partition returns exactly as many
    // plausible rows, just the wrong ones.
    mod child {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "child")]
        pub struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
            pub parent_id: Uuid,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
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
        // `limit + 1`: the same over-fetch `Repo::page` uses, which is what
        // makes `has_more` an observation rather than a guess.
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
        // `pk > after` must be *inside* the ranked subquery: applied outside, a
        // parent's rank would count rows the caller already has, so page 2
        // would come back short.
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
        // An unparseable cursor must page from the start, not fail the request.
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
        // `clamp_page_size` is the single source of truth shared with
        // `PageParams::limit`; a divergence would silently widen the bound.
        assert_eq!(clamp_page_size(0), 1);
        assert_eq!(clamp_page_size(1), 1);
        assert_eq!(clamp_page_size(20), 20);
        assert_eq!(clamp_page_size(100), 100);
        assert_eq!(clamp_page_size(u64::MAX), 100);
    }

    // `split_overfetched` is the boundary between the DB fetch and the
    // page shape: fewer than `limit + 1` rows ⇒ this is the last page; the
    // extra row signals "more to come" and is dropped from the visible items.
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
        // `Page` is a plain public data carrier — every field reachable so
        // a helper outside the crate (e.g. a custom paginator) can build one.
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

    // `next_cursor_from` is the cursor-selection branch lifted out of
    // `Repo::page`: a non-last page yields the last row's pk, a terminal
    // page yields `None`. The bug we are pinning here is the symmetrical
    // shape — `has_more = true` with an empty `items` returns `None`
    // (not a panic from `last()`), and `has_more = false` skips the pk
    // closure entirely (no extra DB-side work on a terminal page).
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
        // Defensive: the production extractor `ValueType::try_from` can
        // fail in theory (a type mismatch between Uuid and the column).
        // The page must surface `None`, not crash.
        let next = next_cursor_from(&[1, 2, 3], true, |_| None);
        assert_eq!(next, None);
    }

    #[test]
    fn next_cursor_from_on_empty_with_has_more_is_none() {
        // Pathological: `has_more` true with no items. `items.last()` is
        // `None`, so the cursor is `None` — never a panic from indexing.
        let next = next_cursor_from::<i32>(&[], true, |_| Some(Uuid::now_v7()));
        assert_eq!(next, None);
    }

    #[test]
    fn next_cursor_from_passes_the_last_item_to_the_pk_closure() {
        // Pinning the per-item input the closure receives: only the LAST
        // item — a regression that paged from the first would shift the
        // entire stream by one window.
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
        // The HTTP query extractor relies on `Clone` (echo back in logs) and
        // `Debug` (request-context dump). A regression on either turns the
        // extractor into a compile error far from this file.
        let p = params(Some(10), Some("not-a-uuid"));
        let cloned = p.clone();
        assert_eq!(cloned.first, Some(10));
        assert_eq!(cloned.after.as_deref(), Some("not-a-uuid"));
        let _ = format!("{p:?}");
    }
}
