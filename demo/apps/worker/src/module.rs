use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_redis::{QueueModule, QueueWorkerModule};
use nest_rs_seaorm::DatabaseModule;

use features::audio::AudioQueueModule;
use features::notifications::NotificationsQueueModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        QueueWorkerModule,
        AudioQueueModule,
        NotificationsQueueModule,
    ],
)]
pub struct WorkerModule;
