use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::HttpModule;
use nest_rs::queue::QueueModule;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisScheduleModule};
use nest_rs::schedule::ScheduleModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};

use features::audio::AudioQueueModule;
use features::notifications::{NotificationsQueueModule, NotificationsScheduleModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        QueueModule::for_root(None),
        ScheduleModule,
        RedisScheduleModule,
        HttpModule::for_root(None),
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
        NotificationsScheduleModule,
    ],
)]
pub struct WorkerModule;
