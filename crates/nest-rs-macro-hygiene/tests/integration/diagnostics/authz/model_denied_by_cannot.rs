//! `cannot` takes an entity, whose rows a denial narrows; a row struct handed
//! in its place — the entity's `Model` — is refused for what it is not, never
//! called a capability.

use nest_rs::authz::{AbilityBuilder, Action};

mod post {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "denied_model_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn main() {
    let mut ab = AbilityBuilder::new();
    ab.cannot(Action::Delete, post::Model { id: Default::default() });
}
