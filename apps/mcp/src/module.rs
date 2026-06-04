use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_http::{HttpConfig, HttpModule};
use nestrs_opentelemetry::OpenTelemetryModule;
use nestrs_server_timing::ServerTimingModule;

use crate::weather::WeatherModule;

#[module(imports = [
    WeatherModule,
    HealthModule,
    OpenTelemetryModule,
    ServerTimingModule,
    HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
])]
pub struct McpModule;
