use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_redis::{QueueModule, QueueWorkerModule};
use nest_rs_seaorm::DatabaseModule;

use features::audio::AudioQueueModule;
use features::notifications::NotificationsQueueModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        QueueWorkerModule,
        HttpModule::for_root(HttpConfig { port: 3005, ..Default::default() }),
        HealthModule,
        AudioQueueModule,
        NotificationsQueueModule,
    ],
)]
pub struct WorkerModule;
