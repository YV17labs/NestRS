use nestrs_authn::AuthnModule;
use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_database::DatabaseModule;
use nestrs_graphql::GraphqlModule;
use nestrs_health::HealthModule;
use nestrs_openapi::OpenApiModule;
use nestrs_server_timing::ServerTimingModule;
use nestrs_telemetry::TelemetryModule;

use crate::authn::AuthnModule as LocalAuthnModule;
use crate::authz::AuthzModule;
use crate::orgs::OrgsModule;
use crate::users::UsersModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(),
        AuthnModule::for_root(),
        LocalAuthnModule,
        AuthzModule,
        OrgsModule,
        UsersModule,
        GraphqlModule::for_root(),
        HealthModule,
        OpenApiModule::for_root(),
        TelemetryModule,
        ServerTimingModule,
    ],
)]
pub struct AppModule;
