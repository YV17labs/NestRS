//! An `Option` column already answers `null` where a grant withholds it: one way
//! to say a thing, so `nullable` on it is refused.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing")]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(nullable)]
    pub nickname: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

fn main() {}
