use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Append-only notification row. HTTP-only (no `graphql`), read-only: the wire
/// contract exposes every column, so no `Create`/`Update` input is derived and
/// no column is hidden (nothing needs a hand-written `WireModelDefaults`).
/// `org_id` is exposed because the ability rule predicates row-level scope on
/// it.
#[expose(name = "Notification", service = super::service::NotificationsService)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(
    table_name = "notification",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[expose]
    pub message: String,
    #[expose]
    pub created_at: DateTimeWithTimeZone,
}

// No `timestamps`/`soft_delete` flags (append-only, write-once), so the
// lifecycle hooks that would supply this aren't generated — declare the default
// behaviour explicitly, as a plain non-flagged entity does.
impl ActiveModelBehavior for ActiveModel {}
