use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct HelloDto {
    pub message: String,
}

#[derive(Serialize, JsonSchema)]
pub struct ItemDto {
    pub id: u32,
    pub message: String,
}
