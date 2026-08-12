//! The dataloaders an exposed relation needs are emitted *on the service*, so
//! the macro has to be told which type that is — there is nowhere else to hang
//! a `#[dataloader]` impl.

use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;

#[expose(name = "Org", graphql)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "orgs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[sea_orm(has_many)]
    #[expose]
    pub posts: HasMany<crate::posts::Entity>,
}

fn main() {}
