//! `soft_delete` stamps a column; without it the flag would emit a
//! `SoftDeletable` impl naming a column the entity does not have, and a delete
//! would fail at runtime instead of at build time.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, soft_delete)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub name: String,
}

fn main() {}
