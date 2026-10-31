//! A denial holds whatever the token carries: `requires_scope` withholds a
//! grant from a narrower token, and a denial withheld that way would widen it.

use nest_rs::authz::{AbilityBuilder, Action};
use nest_rs::guards::Scope;

const POSTS_ADMIN: Scope = Scope::new("posts:admin");

mod posts {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "scoped_denial_posts")]
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
    ab.cannot(Action::Delete, posts::Entity)
        .requires_scope(POSTS_ADMIN);
}
