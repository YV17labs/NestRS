use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_server_timing::ServerTimingModule;
use nestrs_telemetry::TelemetryModule;

use crate::weather::WeatherModule;

#[module(imports = [WeatherModule, HealthModule, TelemetryModule, ServerTimingModule])]
pub struct AppModule;
