use serde::{Deserialize, Serialize};

use crate::Subject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageDto {
    pub author: Subject,
    pub text: String,
}
