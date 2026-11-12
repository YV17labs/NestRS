//! The primary key is the insert's to seed: a `Uuid` with a v7 UUID, any other
//! by the database.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose(default = Uuid::nil())]
    pub id: Uuid,
    #[expose]
    pub name: String,
}

fn main() {}
