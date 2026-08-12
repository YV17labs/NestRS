//! `complex` asks for a `#[ComplexObject]`, which is an async-graphql shape:
//! without `graphql` the wire DTO is a plain serde struct with nothing to hang
//! it on.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, complex)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
}

fn main() {}
