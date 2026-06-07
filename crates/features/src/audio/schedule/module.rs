use nest_rs_core::module;

use super::producer::AudioTasks;
use crate::audio::AudioModule;

#[module(imports = [AudioModule], providers = [AudioTasks])]
pub struct AudioScheduleModule;
