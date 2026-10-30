use nest_rs::core::module;
use nest_rs::queue::QueueModule;

use super::processor::AudioProcessor;
use crate::audio::AudioModule;

#[module(imports = [AudioModule, QueueModule], providers = [AudioProcessor])]
pub struct AudioQueueModule;
