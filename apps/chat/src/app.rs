use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_telemetry::TelemetryModule;

use crate::chat::ChatModule;

#[module(imports = [ChatModule, HealthModule, TelemetryModule])]
pub struct AppModule;
