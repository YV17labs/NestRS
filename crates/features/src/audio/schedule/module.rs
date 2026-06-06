use nest_rs_core::module;

use super::producer::AudioTasks;

#[module(providers = [AudioTasks])]
pub struct AudioScheduleModule;
