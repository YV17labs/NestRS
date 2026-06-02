use nestrs_authn::{AuthnModule, OAuth2Module};
use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_database::DatabaseModule;
use nestrs_health::HealthModule;
use nestrs_telemetry::TelemetryModule;
use nestrs_throttler::{Throttle, ThrottlerModule};

use crate::oauth::OAuthModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        AuthnModule::for_root(None),
        OAuth2Module::for_root(None),
        ThrottlerModule::for_root(Throttle::per_minute(60)),
        OAuthModule,
        HealthModule,
        TelemetryModule,
    ],
)]
pub struct AppModule;
