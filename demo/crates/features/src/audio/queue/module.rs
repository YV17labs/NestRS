use nest_rs::core::module;

use super::processor::AudioProcessor;
use crate::audio::AudioModule;

#[module(imports = [AudioModule], providers = [AudioProcessor])]
pub struct AudioQueueModule;
