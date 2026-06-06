use nest_rs_core::module;

use super::processor::AudioJobs;
use crate::audio::AudioModule;

#[module(imports = [AudioModule], providers = [AudioJobs])]
pub struct AudioQueueModule;
