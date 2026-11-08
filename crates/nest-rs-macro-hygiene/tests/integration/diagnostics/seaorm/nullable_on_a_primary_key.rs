//! The primary key names its row, which no grant withholds: `nullable` on it
//! is refused naming that.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing")]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose(nullable)]
    pub id: Uuid,
    #[expose]
    pub name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

fn main() {}
