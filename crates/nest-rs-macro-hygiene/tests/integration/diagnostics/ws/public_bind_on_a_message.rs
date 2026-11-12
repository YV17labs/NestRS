//! A message names no id, so `bind = Service` is refused on `#[public(...)]` in
//! the words `#[authorize]` meets there, which name the body's `access` load.

// The macro bails before emitting the item, so every import is unused by the
// time rustc looks: noise the snapshot should not carry.
#![allow(unused_imports)]

use nest_rs::authz::Read;
use nest_rs::ws::{gateway, messages};

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

struct PostsService;

#[gateway(path = "/ws")]
struct PostsGateway;

#[messages]
impl PostsGateway {
    #[subscribe_message("one")]
    #[public(Read, posts::Entity, bind = PostsService)]
    async fn one(&self) -> String {
        String::new()
    }
}

fn main() {}
