use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessTokenDto {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}
