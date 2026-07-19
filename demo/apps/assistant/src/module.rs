use features::audio::AudioMcpModule;
use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_redis::QueueModule;
use nest_rs_server_timing::ServerTimingModule;

#[module(
    imports = [
        OpenTelemetryModule,
        ServerTimingModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
        QueueModule::for_root(None),
        AudioMcpModule,
    ],
)]
pub struct AssistantModule;
