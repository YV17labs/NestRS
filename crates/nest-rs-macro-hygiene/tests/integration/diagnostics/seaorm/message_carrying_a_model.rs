//! A message answering a row, or reading one off its payload, says what crosses a
//! wire, as a route does: a reply answers the exposed type and a write takes an input.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose(input(create, update))]
        pub title: String,
        pub author_email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::ws::{gateway, messages};

#[gateway(path = "/posts")]
pub struct PostsGateway;

#[messages]
impl PostsGateway {
    #[subscribe_message("post.answered")]
    #[public]
    async fn answered(&self) -> post::Model {
        loop {}
    }

    #[subscribe_message("post.read")]
    #[public]
    async fn read(&self, _row: post::Model) -> i32 {
        1
    }
}

fn main() {}
