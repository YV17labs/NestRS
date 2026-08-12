//! A live row has no deletion stamp, so the column has to be nullable — a
//! non-`Option` one would make "deleted" unrepresentable and the emitted
//! `SoftDeletable` impl a lie.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, soft_delete)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    pub deleted_at: DateTimeWithTimeZone,
}

fn main() {}
