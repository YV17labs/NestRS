//! A `Repo` select hands out no statement: sea-orm's `query()` is a method of
//! builder traits it does not implement, so what `Repo` filtered stays filtered.

use nest_rs::authz::Action;
use nest_rs::seaorm::Repo;
use sea_orm::entity::prelude::*;
use sea_orm::QueryTrait;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

fn main() {
    let mut select = Repo::<Entity>::scoped(Action::Read);
    *select.query() = Entity::find().into_query();
}
