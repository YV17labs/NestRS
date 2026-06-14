use schemars::JsonSchema;
use serde::Serialize;

/// RFC 6749 bearer-token response envelope.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessTokenDto {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}
