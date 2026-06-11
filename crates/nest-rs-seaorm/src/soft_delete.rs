//! Opt-in soft-delete markers and read filters.
//!
//! Entities declare [`SoftDeletable`] via `#[expose(..., soft_delete)]`; services
//! opt in through [`CrudService::soft_delete_column`](crate::CrudService::soft_delete_column).
//! Hand-written queries that bypass `CrudService` should AND
//! [`live_condition`](live_condition) onto [`Repo::scoped`](crate::Repo::scoped).

use sea_orm::sea_query::Condition;
use sea_orm::{ColumnTrait, EntityTrait};

/// Marker for entities with a nullable `deleted_at` tombstone column. Emitted by
/// `#[expose(..., soft_delete)]`; the service still opts in via
/// [`CrudService::soft_delete_column`](crate::CrudService::soft_delete_column).
pub trait SoftDeletable: EntityTrait {
    fn deleted_at_column() -> Self::Column;
}

/// `deleted_at IS NULL` for a [`SoftDeletable`] entity — AND this onto any custom
/// [`Repo::scoped`](crate::Repo::scoped) query so tombstones stay invisible.
pub fn live_condition<E: SoftDeletable>() -> Condition {
    live_condition_for_column(E::deleted_at_column())
}

/// The `<col> IS NULL` live-row predicate, built from a tombstone column — the
/// single source of "what a live row looks like" shared by [`live_condition`]
/// and `CrudService::live_read_filter`.
pub(crate) fn live_condition_for_column<C: ColumnTrait>(col: C) -> Condition {
    Condition::all().add(col.is_null())
}
