use schemars::JsonSchema;
use serde::Deserialize;
use validator::Validate;

#[derive(Debug, Deserialize, Validate, JsonSchema)]
pub struct LoginInput {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8, message = "password must be at least 8 characters"))]
    pub password: String,
}
