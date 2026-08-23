use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisWorkerModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule, SeaOrmModule};

use features::audio::AudioQueueModule;
use features::notifications::NotificationsQueueModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        SeaOrmHealthModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        RedisWorkerModule::for_root(None),
        HttpModule::for_root(HttpConfig { port: 3005, ..Default::default() }),
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
    ],
)]
pub struct WorkerModule;
