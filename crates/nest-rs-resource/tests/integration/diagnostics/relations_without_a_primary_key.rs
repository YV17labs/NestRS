//! Every relation resolves through a key loader, and the key is the entity's
//! primary key. An entity that declares none has nothing for the inverse side
//! to load it by.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Org", service = OrgsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "orgs")]
pub struct Model {
    #[expose]
    pub slug: String,
    #[sea_orm(has_many)]
    #[expose]
    pub posts: HasMany<crate::posts::Entity>,
}

fn main() {}
