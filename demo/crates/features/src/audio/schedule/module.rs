use nest_rs::core::module;

use super::tasks::AudioTasks;
use crate::audio::AudioModule;

#[module(imports = [AudioModule], providers = [AudioTasks])]
pub struct AudioScheduleModule;
