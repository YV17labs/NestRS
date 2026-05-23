use std::sync::Arc;

use async_graphql::Result;
use nestrs_graphql::resolver;

use crate::users::dto::{CreateUserInput, UserDto};
use crate::users::service::UsersService;

fn to_gql_error(error: impl std::fmt::Display) -> async_graphql::Error {
    async_graphql::Error::new(error.to_string())
}

#[resolver]
pub struct UsersResolver {
    #[inject]
    users: Arc<UsersService>,
}

#[resolver]
impl UsersResolver {
    #[query]
    async fn users(&self) -> Vec<UserDto> {
        self.users.list().await
    }

    #[query]
    async fn user(&self, id: String) -> Result<Option<UserDto>> {
        self.users.find(&id).await.map_err(to_gql_error)
    }

    #[mutation]
    async fn create_user(&self, input: CreateUserInput) -> Result<UserDto> {
        self.users.create(input).await.map_err(to_gql_error)
    }
}
