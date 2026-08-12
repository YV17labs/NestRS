//! The inverse of `has_one_without_its_sea_orm_marker`: the type marker and the
//! ORM marker are two halves of one declaration, and either one alone is a
//! mistake worth naming.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Org", service = OrgsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "orgs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub posts: HasMany<crate::posts::Entity>,
}

fn main() {}
