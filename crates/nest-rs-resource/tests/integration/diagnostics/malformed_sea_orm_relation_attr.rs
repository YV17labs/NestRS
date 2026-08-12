//! `#[expose]` reads the entity's own `#[sea_orm(...)]` attributes to find the
//! foreign key, and a parse failure there is surfaced rather than swallowed: a
//! discarded error used to resurface as "`belongs_to` relation needs
//! `#[sea_orm(from = "...")]`" on an attribute that plainly had one.

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
    #[sea_orm(belongs_to, from = org_id, to = "Column::Id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
}

fn main() {}
