use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// REST body for `POST /audio/transcode` — a `Dto` (it crosses the HTTP
/// boundary). The controller hands its `file` to the service, which enqueues a
/// [`super::command::TranscodeCommand`] for the worker.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscodeDto {
    pub file: String,
}
