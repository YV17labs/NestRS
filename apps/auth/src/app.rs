use nestrs_authn::{AuthnModule, OAuth2Module};
use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_telemetry::TelemetryModule;
use nestrs_throttler::{Throttle, ThrottlerModule};

use crate::oauth::OAuthModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        // Env-driven: the JWT key pair + OAuth provider come from NESTRS_AUTHN__*.
        AuthnModule::for_root(),
        OAuth2Module::for_root(),
        ThrottlerModule::for_root(Throttle::per_minute(60)),
        OAuthModule,
        HealthModule,
        TelemetryModule,
    ],
)]
pub struct AppModule;
