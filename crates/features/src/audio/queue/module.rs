use nestrs_core::module;

use super::processor::AudioProcessor;
use crate::audio::core::AudioCoreModule;

#[module(imports = [AudioCoreModule], providers = [AudioProcessor])]
pub struct AudioQueueModule;
