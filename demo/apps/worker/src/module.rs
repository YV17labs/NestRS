use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::queue::QueueModule;
use nest_rs::redis::{RedisHealthModule, RedisModule, RedisQueueModule, RedisScheduleModule};
use nest_rs::schedule::ScheduleModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule, SeaOrmModule};
use nest_rs::storage::StorageHealthModule;

use features::audio::{AudioQueueModule, Transcodes};
use features::notifications::{NotificationsQueueModule, NotificationsScheduleModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        SeaOrmHealthModule,
        StorageHealthModule,
        RedisModule::for_root(None),
        RedisHealthModule,
        RedisHealthModule::<Transcodes>,
        RedisQueueModule,
        QueueModule::for_root(None),
        ScheduleModule,
        RedisScheduleModule,
        HttpModule::for_root(HttpConfig { port: 3005, ..Default::default() }),
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
        NotificationsScheduleModule,
    ],
)]
pub struct WorkerModule;
