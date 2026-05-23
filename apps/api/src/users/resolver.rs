use std::sync::Arc;

use async_graphql::dataloader::DataLoader;
use async_graphql::Result;
use nestrs_graphql::resolver;

use crate::users::dto::{CreateUserInput, UserDto};
use crate::users::service::{UsersService, UsersServiceByName};

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

    #[field]
    async fn namesakes(
        &self,
        parent: &UserDto,
        by_name: &DataLoader<UsersServiceByName>,
    ) -> Result<Vec<UserDto>> {
        // `?` surfaces a real loader error; `unwrap_or_default` only covers the
        // legitimate "no rows for this name" case (`load_one` returns `None`).
        let same_name = by_name
            .load_one(parent.name.clone())
            .await?
            .unwrap_or_default();
        Ok(same_name
            .into_iter()
            .filter(|u| u.id != parent.id)
            .collect())
    }
}
