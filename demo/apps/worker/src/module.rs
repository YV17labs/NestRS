use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::opentelemetry::OpenTelemetryModule;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisScheduleModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};

use features::audio::AudioQueueModule;
use features::notifications::{NotificationsQueueModule, NotificationsScheduleModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        RedisScheduleModule,
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
        NotificationsScheduleModule,
    ],
)]
pub struct WorkerModule;
