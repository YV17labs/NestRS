use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::redis::{QueueModule, QueueWorkerModule};
use nest_rs::seaorm::{DatabaseHealthModule, DatabaseModule};

use features::audio::AudioQueueModule;
use features::notifications::NotificationsQueueModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        DatabaseHealthModule,
        QueueModule::for_root(None),
        QueueWorkerModule,
        HttpModule::for_root(HttpConfig { port: 3005, ..Default::default() }),
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
    ],
)]
pub struct WorkerModule;
