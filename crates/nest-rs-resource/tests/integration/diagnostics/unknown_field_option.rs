//! The per-field option list is closed too, and for the sharper reason: a
//! misspelled `input(...)` would leave the column read-only with no sign.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(readonly)]
    pub name: String,
}

fn main() {}
