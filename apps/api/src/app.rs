use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_database::DatabaseModule;
use nestrs_graphql::GraphqlModule;
use nestrs_health::HealthModule;
use nestrs_openapi::OpenApiModule;
use nestrs_server_timing::ServerTimingModule;
use nestrs_telemetry::TelemetryModule;

use features::authn::AuthnCoreModule;
use features::authz::{AuthzGraphqlModule, AuthzHttpModule, AuthzWsModule};
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule, UsersWsModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        AuthnCoreModule,
        AuthzHttpModule,
        AuthzGraphqlModule,
        AuthzWsModule,
        OrgsHttpModule,
        OrgsGraphqlModule,
        UsersHttpModule,
        UsersGraphqlModule,
        UsersWsModule,
        GraphqlModule::for_root(None),
        HealthModule,
        OpenApiModule::for_root(None),
        TelemetryModule,
        ServerTimingModule,
    ],
)]
pub struct AppModule;
