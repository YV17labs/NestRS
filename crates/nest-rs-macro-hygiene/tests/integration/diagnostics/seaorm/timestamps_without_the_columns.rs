//! `timestamps` stamps `created_at` from `before_save`, and `updated_at` when
//! the entity has one: an entity with neither is refused, naming the column it
//! needs and the flag to drop.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, timestamps)]
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
