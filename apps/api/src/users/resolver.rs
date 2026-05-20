use std::sync::Arc;

use async_graphql::{Context, Object, Result};
use nestrs_core::Container;

use crate::users::dto::{CreateUserInput, UserDto};
use crate::users::service::UsersService;

/// Resolve the `UsersService` from the per-request GraphQL context.
fn users_service(ctx: &Context<'_>) -> Result<Arc<UsersService>> {
    ctx.data::<Container>()?
        .get()
        .ok_or_else(|| async_graphql::Error::new("UsersService is not registered"))
}

/// GraphQL queries exposed by the users feature.
/// Equivalent of the `@Query()` methods in a NestJS resolver.
#[derive(Default)]
pub struct UsersQuery;

#[Object]
impl UsersQuery {
    async fn users(&self, ctx: &Context<'_>) -> Result<Vec<UserDto>> {
        Ok(users_service(ctx)?.list().await)
    }

    async fn user(&self, ctx: &Context<'_>, id: u32) -> Result<Option<UserDto>> {
        Ok(users_service(ctx)?.find(id).await)
    }
}

/// GraphQL mutations exposed by the users feature.
#[derive(Default)]
pub struct UsersMutation;

#[Object]
impl UsersMutation {
    async fn create_user(&self, ctx: &Context<'_>, input: CreateUserInput) -> Result<UserDto> {
        Ok(users_service(ctx)?.create(input).await)
    }
}
