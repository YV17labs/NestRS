use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;

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
