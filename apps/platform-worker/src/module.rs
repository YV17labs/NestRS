use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_redis::{QueueModule, QueueWorkerModule};

use features::audio::{AudioModule, AudioQueueModule};

#[module(imports = [
    ConfigModule::for_root(),
    QueueModule::for_root(None),
    QueueWorkerModule,
    AudioModule,
    AudioQueueModule,
])]
pub struct PlatformWorkerModule;
