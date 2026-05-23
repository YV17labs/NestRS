use async_graphql::{InputObject, SimpleObject};
use serde::Serialize;
use validator::Validate;

use crate::users::entity::User;

#[derive(Debug, Clone, Serialize, SimpleObject)]
#[graphql(complex)]
pub struct UserDto {
    pub id: String,
    pub name: String,
    pub email: String,
}

impl From<&User> for UserDto {
    fn from(u: &User) -> Self {
        Self {
            id: u.id.clone(),
            name: u.name.clone(),
            email: u.email.clone(),
        }
    }
}

#[derive(Debug, Clone, InputObject, Validate)]
pub struct CreateUserInput {
    #[validate(length(min = 1))]
    pub name: String,
    #[validate(email)]
    pub email: String,
}
