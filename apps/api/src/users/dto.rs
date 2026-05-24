use async_graphql::{InputObject, SimpleObject};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::users::entity;

// `JsonSchema` feeds the OpenAPI document (`OpenApiModule`) the same way
// `SimpleObject`/`InputObject` feed the GraphQL schema — one derive per surface.
// The GraphQL surface returns `UserDto`; the HTTP surface returns the masked
// entity directly (the authz layer strips fields per the caller's ability).
#[derive(Debug, Clone, Serialize, SimpleObject, JsonSchema)]
#[graphql(complex)]
pub struct UserDto {
    pub id: String,
    pub name: String,
    pub email: String,
}

impl From<&entity::Model> for UserDto {
    fn from(u: &entity::Model) -> Self {
        Self {
            id: u.id.to_string(),
            name: u.name.clone(),
            email: u.email.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, InputObject, Validate, JsonSchema)]
pub struct CreateUserInput {
    #[validate(length(min = 1))]
    pub name: String,
    #[validate(email)]
    pub email: String,
}
