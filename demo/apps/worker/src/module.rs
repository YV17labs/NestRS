use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisScheduleModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};

use features::audio::AudioQueueModule;
use features::notifications::{NotificationsQueueModule, NotificationsScheduleModule};

#[module(
    imports = [
        SeaOrmModule,
        SeaOrmDatabaseModule,
        RedisModule,
        RedisQueueModule,
        RedisScheduleModule,
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
        NotificationsScheduleModule,
    ],
)]
pub struct WorkerModule;
