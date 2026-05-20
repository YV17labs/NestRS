use async_graphql::{InputObject, SimpleObject};
use serde::Serialize;

use crate::users::entity::User;

/// GraphQL output type — what clients receive.
#[derive(Debug, Clone, Serialize, SimpleObject)]
pub struct UserDto {
    pub id: u32,
    pub name: String,
    pub email: String,
}

impl From<&User> for UserDto {
    fn from(u: &User) -> Self {
        Self {
            id: u.id,
            name: u.name.clone(),
            email: u.email.clone(),
        }
    }
}

/// GraphQL input type — argument of the `createUser` mutation.
#[derive(Debug, Clone, InputObject)]
pub struct CreateUserInput {
    pub name: String,
    pub email: String,
}
