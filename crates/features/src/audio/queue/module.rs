use nestrs_core::module;

use super::processor::AudioJobs;
use crate::audio::core::AudioCoreModule;

#[module(imports = [AudioCoreModule], providers = [AudioJobs])]
pub struct AudioQueueModule;
