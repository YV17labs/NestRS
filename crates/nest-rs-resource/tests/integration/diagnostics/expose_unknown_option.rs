//! The option list is closed, and a typo has to name the options rather than be
//! ignored: an ignored `graphql` would silently ship an entity with no GraphQL
//! surface at all.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, cached)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
}

fn main() {}
