use nest_rs::authn::AuthnModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::opentelemetry::OpenTelemetryModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule};
use nest_rs::social::SocialModule;
use nest_rs::throttler::ThrottlerModule;

use features::app_oauth::AppOAuthHttpModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        SeaOrmDatabaseModule::for_root(None),
        SeaOrmHealthModule,
        ThrottlerModule::for_root(None),
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3001, ..Default::default() }),
        AuthnModule::for_root(None),
        SocialModule,
        AppOAuthHttpModule,
    ],
)]
pub struct AuthModule;
