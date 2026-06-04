use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_http::{HttpConfig, HttpModule};
use nestrs_telemetry::TelemetryModule;

use crate::chat::ChatModule;
use crate::notify::NotifyModule;

#[module(imports = [
    ChatModule,
    NotifyModule,
    HealthModule,
    TelemetryModule,
    HttpModule::for_root(HttpConfig { port: 3004, ..Default::default() }),
])]
pub struct AppModule;
