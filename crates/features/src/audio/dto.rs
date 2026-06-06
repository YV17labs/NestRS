use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Queue name shared by the producer (the `api` app) and the consumer's
/// `#[processor]` (the `worker` app). Stringly-typed — both sides must agree.
pub const AUDIO_QUEUE: &str = "audio";

/// The job payload exchanged over the `audio` queue. Lives in the feature's
/// `core` so producer and consumer apps share one contract via the crate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscodeJob {
    pub file: String,
}
