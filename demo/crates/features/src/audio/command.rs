use nest_rs_queue::{QueueName, queue};
use serde::{Deserialize, Serialize};

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

/// Typed handle for the `audio` queue — the single artifact both sides import.
/// The producer (`AudioService::enqueue_transcode`) pushes with
/// `push_to::<AudioQueue>` and the consumer ([`super::AudioProcessor`]) drains
/// with `#[process(queue = AudioQueue)]`; the name and the [`TranscodeCommand`]
/// payload are compile-checked on both ends.
#[queue(name = "audio", job = TranscodeCommand)]
pub struct AudioQueue;

/// The queue's wire name, sourced from [`AudioQueue`] so the literal `"audio"`
/// lives in exactly one place. Kept for call sites that want the bare string
/// (e.g. a log field).
pub const AUDIO_QUEUE: &str = <AudioQueue as QueueName>::NAME;
