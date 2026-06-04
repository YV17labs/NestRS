use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_http::{HttpConfig, HttpModule};
use nestrs_server_timing::ServerTimingModule;
use nestrs_telemetry::TelemetryModule;

use crate::weather::WeatherModule;

#[module(imports = [
    WeatherModule,
    HealthModule,
    TelemetryModule,
    ServerTimingModule,
    HttpModule::for_root(HttpConfig { port: 3003, ..Default::default() }),
])]
pub struct AppModule;
