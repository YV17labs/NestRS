use nestrs_core::module;

use super::producer::AudioTasks;

#[module(providers = [AudioTasks])]
pub struct AudioScheduleModule;
