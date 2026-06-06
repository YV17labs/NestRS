use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_server_timing::ServerTimingModule;

use crate::weather::WeatherModule;

#[module(imports = [
    WeatherModule,
    HealthModule,
    OpenTelemetryModule,
    ServerTimingModule,
    HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
])]
pub struct McpModule;
