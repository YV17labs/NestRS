use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TranscodeState {
    Pending,
    Ready,
    Error,
}
