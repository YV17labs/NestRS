use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TokenRequestDto {
    pub grant_type: String,
    #[serde(default)]
    pub scope: Option<String>,
}
