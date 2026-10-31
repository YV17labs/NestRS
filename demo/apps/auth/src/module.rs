use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::opentelemetry::OpenTelemetryModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::social::{GithubSocialModule, GoogleSocialModule};
use nest_rs::throttler::ThrottlerModule;

use features::sign_in::SignInHttpModule;

#[module(
    imports = [
        OpenTelemetryModule,
        SeaOrmModule,
        SeaOrmDatabaseModule,
        ThrottlerModule,
        HealthModule,
        GithubSocialModule,
        GoogleSocialModule,
        SignInHttpModule,
    ],
)]
pub struct AuthModule;
