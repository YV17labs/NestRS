use nestrs_auth::{AuthModule, JwtOptions, OAuth2Config, OAuth2Module};
use nestrs_config::env_var;
use nestrs_core::module;
use nestrs_health::HealthModule;
use nestrs_telemetry::TelemetryModule;
use nestrs_throttler::{Throttle, ThrottlerModule};

use crate::oauth::OAuthModule;

const DEV_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";

#[module(
    imports = [
        AuthModule::for_root(JwtOptions::eddsa(
            env_var("JWT_PRIVATE_KEY").unwrap_or_else(|| DEV_PRIVATE_KEY_PEM.into()),
            env_var("JWT_PUBLIC_KEY").unwrap_or_else(|| identity::DEV_PUBLIC_KEY_PEM.into()),
        )),
        OAuth2Module::for_root(OAuth2Config {
            client_id: env_var("OAUTH_CLIENT_ID").unwrap_or_else(|| "demo-client-id".into()),
            client_secret: env_var("OAUTH_CLIENT_SECRET")
                .unwrap_or_else(|| "demo-client-secret".into()),
            auth_url: "https://github.com/login/oauth/authorize".into(),
            token_url: "https://github.com/login/oauth/access_token".into(),
            userinfo_url: "https://api.github.com/user".into(),
            redirect_url: env_var("OAUTH_REDIRECT_URL")
                .unwrap_or_else(|| "http://localhost:3002/callback".into()),
            scopes: vec!["read:user".into()],
        }),
        ThrottlerModule::for_root(Throttle::per_minute(60)),
        OAuthModule,
        HealthModule,
        TelemetryModule,
    ],
)]
pub struct AppModule;
