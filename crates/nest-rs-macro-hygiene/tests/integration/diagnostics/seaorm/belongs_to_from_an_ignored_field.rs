//! `from = "…"` names the field holding the foreign key, which a loader keys
//! on through its `Column` variant: an `ignore`d field has none, so it is
//! refused at SeaORM's key rather than as a variant missing inside a query.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Post", service = PostsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[sea_orm(ignore)]
    #[expose]
    pub org_id: Uuid,
    #[sea_orm(belongs_to, from = "org_id", to = "Column::Id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}
