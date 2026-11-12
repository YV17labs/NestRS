//! A key `#[public(Action, Subject)]` cannot take is refused naming what it
//! accepts, never parsed and dropped.

// The macro bails before emitting the item, so every import is unused by the
// time rustc looks: noise the snapshot should not carry.
#![allow(unused_imports)]

use nest_rs::authz::Read;
use nest_rs::graphql::async_graphql::Result;
use nest_rs::graphql::{operations, resolver};

mod posts {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "public_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[resolver]
struct PostsResolver;

#[operations]
impl PostsResolver {
    #[query]
    #[public(Read, posts::Entity, audience = "everyone")]
    async fn list(&self) -> Result<String> {
        Ok(String::new())
    }
}

fn main() {}
