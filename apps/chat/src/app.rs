use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_telemetry::TelemetryModule;

use crate::chat::ChatModule;
use crate::notify::NotifyModule;

#[module(imports = [ChatModule, NotifyModule, HealthModule, TelemetryModule])]
pub struct AppModule;
