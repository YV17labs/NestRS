use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::queue::QueueModule;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisScheduleModule};
use nest_rs::schedule::ScheduleModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule, SeaOrmModule};

use features::audio::AudioQueueModule;
use features::notifications::{NotificationsQueueModule, NotificationsScheduleModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        SeaOrmHealthModule,
        RedisModule::for_root(None),
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
