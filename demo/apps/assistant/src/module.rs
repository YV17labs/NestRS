use features::audio::AudioMcpModule;
use features::posts::PostsMcpModule;
use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_redis::QueueModule;
use nest_rs_seaorm::DatabaseModule;
use nest_rs_server_timing::ServerTimingModule;

#[module(
    imports = [
        OpenTelemetryModule,
        ServerTimingModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        AudioMcpModule,
        PostsMcpModule,
    ],
)]
pub struct AssistantModule;
