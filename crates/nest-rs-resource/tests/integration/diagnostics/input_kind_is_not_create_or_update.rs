//! `input(...)` names the write DTOs a column joins, and there are exactly two.
//! A third name is a column silently absent from both.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(input(create, replace))]
    pub name: String,
}

fn main() {}
