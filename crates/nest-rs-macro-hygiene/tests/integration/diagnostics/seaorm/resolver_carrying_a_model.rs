//! An operation answering a row, or reading one as an argument, says what crosses
//! the wire: a reply answers the exposed type and a write takes an input.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        pub author_email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::graphql::{operations, resolver};

#[resolver]
pub struct PostsResolver;

#[operations]
impl PostsResolver {
    #[query]
    #[public]
    fn answered(&self) -> post::Model {
        loop {}
    }

    #[mutation]
    #[public]
    fn read(&self, _row: post::Model) -> i32 {
        1
    }
}

fn main() {}
