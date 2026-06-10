use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const AUDIO_QUEUE: &str = "audio";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscodeJob {
    pub file: String,
}
