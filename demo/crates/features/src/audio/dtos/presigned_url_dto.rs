use std::collections::BTreeMap;

use nest_rs::storage::PresignedRequest;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct PresignedUrlDto {
    pub key: String,
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
}

impl PresignedUrlDto {
    pub fn signed(key: String, request: PresignedRequest) -> Self {
        Self {
            key,
            method: request.method.to_string(),
            headers: request.headers.into_iter().collect(),
            url: request.url,
        }
    }
}
