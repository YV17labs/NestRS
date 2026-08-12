//! `name` is what the wire DTO, the OpenAPI schema and the GraphQL object are
//! all called, so there is nothing to default it to.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
}

fn main() {}
