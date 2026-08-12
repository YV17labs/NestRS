//! One way to do a thing: the FK-owning side already names its column in
//! `#[sea_orm(belongs_to, from = "…")]`, so `via` there is a second spelling of
//! one fact. The refusal quotes the column it already found.

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
    #[expose(via = "org_id")]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}
