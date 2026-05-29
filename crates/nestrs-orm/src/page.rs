//! Keyset (cursor) pagination over the primary key.
//!
//! [`Repo::page`](crate::Repo::page) pages a collection by its primary key in
//! ascending order, the same ambient-ability scoping as [`Repo::all`](crate::Repo::all)
//! applied to each page. Keyset beats offset for a feed: it is O(1) on the index
//! rather than O(offset), and stable under concurrent inserts. It is also *free*
//! for this framework's UUID-v7 keys — they are time-ordered, so paging by the
//! key is chronological with no extra sort column.
//!
//! The cursor is the last row's primary key. A surface (`#[crud]`'s generated
//! list) hands the client the [`Page::next_cursor`] when [`Page::has_more`], and
//! the client returns it as `after` to fetch the next page.

use sea_orm::prelude::Uuid;
use sea_orm::sea_query::ValueType;
use sea_orm::{
    DbErr, EntityTrait, Iterable, ModelTrait, PrimaryKeyToColumn, PrimaryKeyTrait, QueryFilter,
};

use nestrs_authz::Action;

use crate::repo::{scope_for, Repo};

/// One keyset page: the rows, the cursor to fetch the next page (the last row's
/// primary key, present only when [`has_more`](Page::has_more)), and whether a
/// further page exists.
pub struct Page<M> {
    pub items: Vec<M>,
    pub next_cursor: Option<Uuid>,
    pub has_more: bool,
}

/// The `?first=&after=` cursor query a paginated list handler reads. `first` is
/// the page size (defaulted and clamped by [`limit`](PageParams::limit)); `after`
/// is the opaque cursor from a prior page (a UUID v7; an unparsable value is
/// ignored, paging from the start).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PageParams {
    pub first: Option<u64>,
    pub after: Option<String>,
}

impl PageParams {
    /// The page size, defaulting to 20 and clamped to `1..=100`.
    pub fn limit(&self) -> u64 {
        self.first.unwrap_or(20).clamp(1, 100)
    }

    /// The `after` cursor parsed as a UUID, or `None` (absent or unparsable).
    pub fn after_uuid(&self) -> Option<Uuid> {
        self.after.as_deref().and_then(|s| Uuid::parse_str(s).ok())
    }
}

impl<E: EntityTrait> Repo<E>
where
    E::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    E::Model: Send + Sync,
{
    /// A keyset page of the rows the caller may [`Read`](Action::Read), ordered by
    /// ascending primary key and starting after the `after` cursor. Fetches one
    /// extra row to decide [`Page::has_more`] and the [`Page::next_cursor`], then
    /// returns at most `first` rows.
    pub async fn page(first: u64, after: Option<Uuid>) -> Result<Page<E::Model>, DbErr> {
        let conn = Self::conn()?;
        let limit = first.clamp(1, 100);

        // The single primary-key column drives the keyset (UUID v7 → chronological).
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

        let mut items = cursor.all(&conn).await?;
        let has_more = items.len() as u64 > limit;
        items.truncate(limit as usize);

        let next_cursor = if has_more {
            items.last().and_then(|model| {
                <Uuid as ValueType>::try_from(ModelTrait::get(model, pk_col)).ok()
            })
        } else {
            None
        };

        Ok(Page {
            items,
            next_cursor,
            has_more,
        })
    }
}
