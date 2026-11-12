//! A GraphQL object is a wire of its own: an async-graphql derive on the row writes
//! every column and reads each one off a body no input validates, and
//! each is refused where it is written, as serde's is, naming what answers instead.

mod post {
    use async_graphql::{InputObject, SimpleObject};
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, SimpleObject, InputObject)]
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

fn main() {}
