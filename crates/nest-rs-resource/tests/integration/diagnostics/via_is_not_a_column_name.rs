//! `via` takes the child's foreign-key **column**, which the macro turns into a
//! marker type. Anything that is not an identifier reports at the string the
//! developer wrote, not at the field or somewhere inside the expansion.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Org", service = OrgsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "orgs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[sea_orm(has_many)]
    #[expose(via = "author id")]
    pub posts: HasMany<crate::posts::Entity>,
}

fn main() {}
