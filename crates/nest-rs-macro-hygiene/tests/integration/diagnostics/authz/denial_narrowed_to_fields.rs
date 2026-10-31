//! A denial hides rows, never columns: `fields` narrows what a grant exposes,
//! and on a denial it would deny nothing it names.

use nest_rs::authz::{AbilityBuilder, Action};

mod users {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "fielded_denial_users")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn main() {
    let mut ab = AbilityBuilder::new();
    ab.cannot(Action::Read, users::Entity)
        .fields([users::Column::Email]);
}
