use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_queue::{QueueModule, QueueWorkerModule};

use features::audio::{AudioCoreModule, AudioQueueModule};

#[module(imports = [
    ConfigModule::for_root(),
    QueueModule::for_root(None),
    QueueWorkerModule,
    AudioCoreModule,
    AudioQueueModule,
])]
pub struct AppModule;
