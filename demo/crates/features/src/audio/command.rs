use nest_rs::queue::{Queue, queue};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeCommand {
    pub file: Uuid,
}

#[queue(name = "audio", job = TranscodeCommand)]
pub struct AudioQueue;

pub const AUDIO_QUEUE: &str = <AudioQueue as Queue>::NAME;
