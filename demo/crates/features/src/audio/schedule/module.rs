use nest_rs::config::ConfigModule;
use nest_rs::core::module;

use super::tasks::AudioTasks;
use crate::audio::{AudioConfig, AudioModule};

#[module(
    imports = [ConfigModule::for_feature::<AudioConfig>(), AudioModule],
    providers = [AudioTasks],
)]
pub struct AudioScheduleModule;
