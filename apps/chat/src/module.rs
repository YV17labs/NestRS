use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_http::{HttpConfig, HttpModule};
use nestrs_opentelemetry::OpenTelemetryModule;

use crate::chat::ChatModule as ChatFeatureModule;
use crate::notify::NotifyModule;

#[module(imports = [
    ChatFeatureModule,
    NotifyModule,
    HealthModule,
    OpenTelemetryModule,
    HttpModule::for_root(HttpConfig { port: 3004, ..Default::default() }),
])]
pub struct ChatModule;
