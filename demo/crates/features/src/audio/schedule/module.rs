use nest_rs::core::module;
use nest_rs::schedule::ScheduleModule;

use super::tasks::AudioTasks;
use crate::audio::AudioModule;

#[module(imports = [AudioModule, ScheduleModule], providers = [AudioTasks])]
pub struct AudioScheduleModule;
