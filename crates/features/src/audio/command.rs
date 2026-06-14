use serde::{Deserialize, Serialize};

pub const AUDIO_QUEUE: &str = "audio";

/// Imperative payload for the `audio` queue — "transcode this file". A
/// **`Command`** (one processor handles it), not a `Dto`: it crosses the
/// producer↔worker boundary, so it lives at the feature port and the
/// `queue/` adapter's processor imports it. The HTTP body that triggers it is
/// a separate `TranscodeDto` ([`super::dto`]) — each boundary speaks its own
/// vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeCommand {
    pub file: String,
}
