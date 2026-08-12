//! A `belongs_to` is resolved through the foreign key it owns, so the macro has
//! to be told which column that is.

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
    #[sea_orm(belongs_to, to = "Column::Id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}
