//! `#[expose(..., graphql)]` in a build without `graphql` names the feature to add
//! on the one line a developer has, the umbrella's, and the attribute they wrote.

use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService, graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
}

fn main() {}
