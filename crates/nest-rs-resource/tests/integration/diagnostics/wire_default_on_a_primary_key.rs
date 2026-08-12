//! A primary key is never fabricated: a placeholder there would be a row
//! identity invented by the masking path.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[wire_default]
    pub id: Uuid,
    #[expose]
    pub name: String,
}

fn main() {}
