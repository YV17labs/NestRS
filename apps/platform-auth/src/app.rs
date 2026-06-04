use nestrs_authn::{AuthnModule, OAuth2Module};
use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_database::DatabaseModule;
use nestrs_health::HealthModule;
use nestrs_http::{HttpConfig, HttpModule};
use nestrs_telemetry::TelemetryModule;
use nestrs_throttler::ThrottlerModule;

use features::oauth::OAuthHttpModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        AuthnModule::for_root(None),
        OAuth2Module::for_root(None),
        ThrottlerModule::for_root(None),
        OAuthHttpModule,
        HealthModule,
        TelemetryModule,
        HttpModule::for_root(HttpConfig { port: 3001, ..Default::default() }),
    ],
)]
pub struct AppModule;
