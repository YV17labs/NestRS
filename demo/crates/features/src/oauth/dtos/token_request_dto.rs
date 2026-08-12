use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TokenRequestDto {
    pub grant_type: String,
    #[serde(default)]
    pub scope: Option<String>,
}
