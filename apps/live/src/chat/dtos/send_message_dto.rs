use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageDto {
    pub author: String,
    pub text: String,
}
