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
        self.first.unwrap_or(20).clamp(1, 100)
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
        let limit = first.clamp(1, 100);

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
