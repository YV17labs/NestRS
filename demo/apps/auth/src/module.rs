use nest_rs::authn::AuthnModule;
use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::opentelemetry::OpenTelemetryModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::social::{GithubSocialModule, GoogleSocialModule, SocialModule};
use nest_rs::throttler::ThrottlerModule;

use features::sign_in::SignInHttpModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        ThrottlerModule::for_root(None),
        HealthModule,
        AuthnModule::for_root(None),
        SocialModule,
        GithubSocialModule,
        GoogleSocialModule,
        SignInHttpModule,
    ],
)]
pub struct AuthModule;
