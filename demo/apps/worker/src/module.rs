use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::redis::{RedisQueueModule, RedisWorkerModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule};

use features::audio::AudioQueueModule;
use features::notifications::NotificationsQueueModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmDatabaseModule::for_root(None),
        SeaOrmHealthModule,
        RedisQueueModule::for_root(None),
        RedisWorkerModule,
        HttpModule::for_root(HttpConfig { port: 3005, ..Default::default() }),
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
    ],
)]
pub struct WorkerModule;
