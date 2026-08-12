//! A relation is materialised by a field resolver, not by a column setter:
//! `input(...)` on one would emit `Set(self.org)` against a `HasOne` marker and
//! fail deep in the generated active-model glue.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Post", service = PostsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[sea_orm(belongs_to, from = "org_id", to = "Column::Id")]
    #[expose(input(create))]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}
