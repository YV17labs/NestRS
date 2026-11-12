use nest_rs::core::output;

use crate::audio::TranscodeState;

#[output]
#[derive(Debug, Clone, Copy)]
pub struct TranscodeProgressDto {
    pub state: TranscodeState,
    pub attempt: u32,
}
