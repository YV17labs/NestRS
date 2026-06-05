//! Keyset (cursor) pagination over the primary key.
//!
//! Keyset beats offset for a feed: O(1) on the index, stable under concurrent
//! inserts. With UUID-v7 keys (time-ordered), paging by the key is also
//! chronological with no extra sort column.

use sea_orm::prelude::Uuid;
use sea_orm::sea_query::ValueType;
use sea_orm::{
    DbErr, EntityTrait, Iterable, ModelTrait, PrimaryKeyToColumn, PrimaryKeyTrait, QueryFilter,
};

use nestrs_authz::Action;

use crate::repo::{Repo, scope_for};

/// One keyset page. `next_cursor` is the last row's primary key, present only
/// when [`has_more`](Page::has_more).
pub struct Page<M> {
    pub items: Vec<M>,
    pub next_cursor: Option<Uuid>,
    pub has_more: bool,
}

impl<M> Page<M> {
    /// Empty page — no items, no cursor, no more rows. Used by callers that
    /// short-circuit before hitting the DB (e.g. a deny-all scope).
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            next_cursor: None,
            has_more: false,
        }
    }
}

/// Clamp the requested page size to the `1..=100` window — the same bound
/// [`PageParams::limit`] applies, kept here so callers passing a `u64` (e.g.
/// the GraphQL pagination input) reuse one source of truth.
pub fn clamp_page_size(first: u64) -> u64 {
    first.clamp(1, 100)
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
    if has_more { items.last().and_then(pk) } else { None }
}

/// The `?first=&after=` cursor query. An unparsable `after` is ignored — paging
/// from the start, never an error.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PageParams {
    pub first: Option<u64>,
    pub after: Option<String>,
}

impl PageParams {
    /// Page size, defaulting to 20 and clamped to `1..=100`.
    pub fn limit(&self) -> u64 {
        clamp_page_size(self.first.unwrap_or(20))
    }

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
    pub async fn page(first: u64, after: Option<Uuid>) -> Result<Page<E::Model>, DbErr> {
        let conn = Self::conn()?;
        let limit = clamp_page_size(first);

        let pk_col = E::PrimaryKey::iter()
            .next()
            .expect("an entity has at least one primary-key column")
            .into_column();

        let mut cursor = E::find()
            .filter(scope_for::<E>(Action::Read))
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(more, "an over-fetched row means there is at least one more page");
    }

    #[test]
    fn split_overfetched_empty_is_a_terminal_empty_page() {
        let (items, more) = split_overfetched::<i32>(vec![], 10);
        assert!(items.is_empty());
        assert!(!more);
    }

    #[test]
    fn empty_page_has_no_cursor_and_no_more() {
        let page: Page<i32> = Page::empty();
        assert!(page.items.is_empty());
        assert_eq!(page.next_cursor, None);
        assert!(!page.has_more);
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
