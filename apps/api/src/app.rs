use nestrs_auth::{AuthModule, JwtOptions};
use nestrs_config::env_var;
use nestrs_core::module;
use nestrs_graphql::{GraphqlModule, GraphqlOptions};
use nestrs_health::HealthModule;
use nestrs_openapi::{OpenApiModule, OpenApiOptions};
use nestrs_orm::{DatabaseModule, DatabaseOptions};
use nestrs_server_timing::ServerTimingModule;
use nestrs_telemetry::TelemetryModule;

use crate::authn::AuthnModule;
use crate::authz::AuthzModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [
        DatabaseModule::for_root(DatabaseOptions {
            url: env_var("DATABASE_URL").unwrap_or_default(),
            ..Default::default()
        }),
        // Resource server: verify-only. `api` holds **only** the public key, so it
        // validates tokens the `auth` app minted but can never sign one. (No
        // OAuth2 / token-issuing modules here — that all lives in `apps/auth`.)
        AuthModule::for_root(JwtOptions::eddsa_verify(
            env_var("JWT_PUBLIC_KEY").unwrap_or_else(|| identity::DEV_PUBLIC_KEY_PEM.into()),
        )),
        AuthnModule,
        AuthzModule,
        OrgsModule,
        UsersModule,
        GraphqlModule::for_root(GraphqlOptions {
            path: "/graphql".into(),
            playground: true,
            schema_path: concat!(env!("CARGO_MANIFEST_DIR"), "/schema.graphql").into(),
            emit_sdl: cfg!(debug_assertions),
        }),
        HealthModule,
        OpenApiModule::for_root(OpenApiOptions {
            title: "nestrs API".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: Some("Demo API built with nestrs".into()),
        }),
        TelemetryModule,
        ServerTimingModule,
    ],
)]
pub struct AppModule;
