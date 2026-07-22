use schemars::JsonSchema;
use serde::Serialize;

use crate::audio::TranscodeState;

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
pub struct TranscodeProgressDto {
    pub state: TranscodeState,
    pub attempt: u32,
}
