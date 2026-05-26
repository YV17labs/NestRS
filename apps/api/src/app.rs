use nestrs_core::module;
use nestrs_graphql::{GraphqlModule, GraphqlOptions};
use nestrs_health::HealthModule;
use nestrs_openapi::{OpenApiModule, OpenApiOptions};
use nestrs_orm::{DatabaseModule, DatabaseOptions};
use nestrs_server_timing::ServerTimingModule;
use nestrs_telemetry::TelemetryModule;

use crate::auth::AuthGuard;
use crate::authz::{AbilityGuard, AppAbility};
use crate::users::UsersModule;

#[module(
    imports = [
        DatabaseModule::for_root(DatabaseOptions {
            url: std::env::var("DATABASE_URL").unwrap_or_default(),
            ..Default::default()
        }),
        UsersModule,
        GraphqlModule::for_root(GraphqlOptions {
            path: "/graphql".into(),
            playground: true,
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
    providers = [AuthGuard, AbilityGuard, AppAbility],
)]
pub struct AppModule;
