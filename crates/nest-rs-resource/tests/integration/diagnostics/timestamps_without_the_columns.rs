//! `timestamps` maintains two columns from `before_save`, and the refusal
//! carries the remedy for the other way this goes wrong: a hand-written
//! `ActiveModelBehavior` the generated one would collide with.

use nest_rs_resource::expose;
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
