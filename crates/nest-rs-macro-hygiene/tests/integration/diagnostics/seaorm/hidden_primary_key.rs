//! A masked reply reads a row's identity off the wire and never fabricates
//! one, so a primary key `#[expose]` leaves hidden is refused at the field.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Post")]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[expose]
    pub title: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

fn main() {}
