use nest_rs_queue::{QueueName, queue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeCommand {
    pub file: String,
}

#[queue(name = "audio", job = TranscodeCommand)]
pub struct AudioQueue;

pub const AUDIO_QUEUE: &str = <AudioQueue as QueueName>::NAME;
