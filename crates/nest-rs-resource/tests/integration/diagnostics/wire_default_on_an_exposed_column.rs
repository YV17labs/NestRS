//! The placeholder exists so masking can rebuild a `Model` from a body that
//! omits the column. An exposed column is in the body, so a placeholder there
//! is either dead or a value that would overwrite a real one — refused either
//! way, because the whole point of the opt-in is that it stays auditable.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    #[wire_default]
    pub name: String,
}

fn main() {}
