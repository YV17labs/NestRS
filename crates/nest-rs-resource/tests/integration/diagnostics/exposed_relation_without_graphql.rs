//! An exposed relation *is* a GraphQL field resolver — there is no REST shape
//! for it. Without the flag the field would silently vanish from every
//! transport, which is the one thing exposure must never do quietly.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Post", service = PostsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[sea_orm(belongs_to, from = "org_id", to = "Column::Id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}
