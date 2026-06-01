use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_queue::QueueModule;

use crate::audio::AudioModule;

#[module(imports = [
    ConfigModule::for_root(),
    QueueModule::for_root(),
    AudioModule,
])]
pub struct AppModule;
