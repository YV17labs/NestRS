//! `#[wire_default]` fed a rebuild no reply runs any more: a wire row is judged on
//! the whole row where it is built, so the attribute is refused naming that.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum Tier {
    #[default]
    #[sea_orm(string_value = "free")]
    Free,
}

#[expose(name = "Account")]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "accounts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[wire_default]
    pub tier: Tier,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

fn main() {}
