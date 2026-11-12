//! A GraphQL field answering a row no `#[expose]` decorates says what crosses a
//! wire, as a route does, whatever holds the row and whatever the row derives.

mod secret {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, async_graphql::SimpleObject)]
    #[graphql(name = "Secret")]
    #[sea_orm(table_name = "secrets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub password_hash: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::graphql::{operations, resolver};

#[resolver]
pub struct SecretsResolver;

#[operations]
impl SecretsResolver {
    #[query]
    #[public]
    fn secret(&self) -> secret::Model {
        loop {}
    }

    #[query]
    #[public]
    async fn secrets(&self) -> async_graphql::Result<Vec<secret::Model>> {
        loop {}
    }
}

fn main() {}
