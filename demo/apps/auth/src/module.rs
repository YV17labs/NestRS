use nest_rs_authn::AuthnModule;
use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_seaorm::DatabaseModule;
use nest_rs_social::SocialModule;
use nest_rs_throttler::ThrottlerModule;

use features::oauth::OAuthHttpModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        DatabaseModule::for_root(None),
        ThrottlerModule::for_root(None),
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3001, ..Default::default() }),
        AuthnModule::for_root(None),
        SocialModule,
        OAuthHttpModule,
    ],
)]
pub struct AuthModule;
