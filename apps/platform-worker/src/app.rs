use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_queue::QueueModule;

use features::audio::{AudioCoreModule, AudioQueueModule};

#[module(imports = [
    ConfigModule::for_root(),
    QueueModule::for_root(None),
    AudioCoreModule,
    AudioQueueModule,
])]
pub struct AppModule;
