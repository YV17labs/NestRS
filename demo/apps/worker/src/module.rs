use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::redis::{RedisQueueModule, RedisScheduleModule};
use nest_rs::seaorm::SeaOrmDatabaseModule;

use features::audio::AudioQueueModule;
use features::notifications::{NotificationsQueueModule, NotificationsScheduleModule};

#[module(
    imports = [
        SeaOrmDatabaseModule,
        RedisQueueModule,
        RedisScheduleModule,
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
        NotificationsScheduleModule,
    ],
)]
pub struct WorkerModule;
