use nest_rs_authn::{AuthnModule, OAuth2Module};
use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_seaorm::DatabaseModule;
use nest_rs_throttler::ThrottlerModule;

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
        OpenTelemetryModule,
        HttpModule::for_root(HttpConfig { port: 3001, ..Default::default() }),
    ],
)]
pub struct PlatformAuthModule;
