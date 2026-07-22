use schemars::JsonSchema;
use serde::Serialize;

/// Where a transcode job is in its lifecycle. A state enum, not a transfer
/// object — `TranscodeEventDto` carries it across the wire, and the service
/// drives it as the job retries.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TranscodeState {
    Pending,
    Ready,
    Error,
}
