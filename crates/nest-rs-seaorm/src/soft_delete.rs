use std::any::TypeId;
use std::collections::HashMap;
use std::sync::OnceLock;

use sea_orm::sea_query::Condition;
use sea_orm::{ColumnTrait, EntityTrait, IdenStatic, Iterable};

/// An entity with a nullable `deleted_at` tombstone column, emitted by
/// `#[expose(..., soft_delete)]`, which also registers the entity so that
/// every [`Repo`](crate::Repo) read and update hides its tombstoned rows.
///
/// Sealed: only the decorator implements it, since an impl without the
/// registration would leave `Repo` showing tombstones and `Deletable::delete`
/// erasing rows.
///
/// ```compile_fail,E0277
/// use nest_rs_seaorm::SoftDeletable;
/// use nest_rs_seaorm::sea_orm;
/// use sea_orm::entity::prelude::*;
///
/// #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #[sea_orm(table_name = "notes")]
/// pub struct Model {
///     #[sea_orm(primary_key)]
///     pub id: i32,
///     pub deleted_at: Option<DateTimeWithTimeZone>,
/// }
///
/// #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// pub enum Relation {}
///
/// impl ActiveModelBehavior for ActiveModel {}
///
/// impl SoftDeletable for Entity {
///     fn deleted_at_column() -> Column {
///         Column::DeletedAt
///     }
/// }
/// ```
pub trait SoftDeletable: EntityTrait + SoftDeleteDeclared {
    /// The nullable tombstone column (`deleted_at`) whose non-null value marks a
    /// row as soft-deleted.
    fn deleted_at_column() -> Self::Column;
}

/// The seal of [`SoftDeletable`]: implemented by the
/// `#[expose(..., soft_delete)]` expansion beside its registration, never by
/// hand.
pub trait SoftDeleteDeclared {}

/// `deleted_at IS NULL` for a [`SoftDeletable`] entity, table-qualified: the
/// condition every [`Repo`](crate::Repo) read of it already carries, for a
/// query built on [`Repo::with_tombstones`](crate::Repo::with_tombstones) or
/// outside `Repo`.
pub fn live_condition<E: SoftDeletable>() -> Condition {
    Condition::all().add(E::deleted_at_column().is_null())
}

/// One `#[expose(..., soft_delete)]` entity, submitted at link time so that
/// `Repo` can tell a soft-deletable entity without naming its trait; emitted
/// by the decorator, never by hand.
///
/// Fn pointers, since each value is a call and the entry must be a `static`.
pub struct SoftDeleteRegistration {
    /// `TypeId::of::<Entity>()`.
    pub entity: fn() -> TypeId,
    /// The tombstone column's name, as the entity's
    /// [`SoftDeletable::deleted_at_column`] spells it in SQL.
    pub column: fn() -> &'static str,
}

inventory::collect!(SoftDeleteRegistration);

/// Every registered entity's tombstone column, read once from the inventory.
fn registry() -> &'static HashMap<TypeId, &'static str> {
    static REGISTRY: OnceLock<HashMap<TypeId, &'static str>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        inventory::iter::<SoftDeleteRegistration>
            .into_iter()
            .map(|entry| ((entry.entity)(), (entry.column)()))
            .collect()
    })
}

/// `E`'s tombstone column when `E` is declared `soft_delete`, else `None`.
pub(crate) fn tombstone_column<E: EntityTrait>() -> Option<E::Column> {
    let name = *registry().get(&TypeId::of::<E>())?;
    E::Column::iter().find(|column| column.as_str() == name)
}

/// The condition hiding `E`'s tombstoned rows, or `None` when `E` keeps none.
pub(crate) fn live_condition_of<E: EntityTrait>() -> Option<Condition> {
    tombstone_column::<E>().map(|column| Condition::all().add(column.is_null()))
}
