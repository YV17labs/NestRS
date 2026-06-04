use nestrs_config::ConfigModule;
use nestrs_core::module;
use nestrs_database::DatabaseModule;
use nestrs_graphql::GraphqlModule;
use nestrs_health::HealthModule;
use nestrs_http::{HttpConfig, HttpModule};
use nestrs_openapi::OpenApiModule;
use nestrs_opentelemetry::OpenTelemetryModule;
use nestrs_queue::QueueModule;
use nestrs_schedule::ScheduleModule;
use nestrs_server_timing::ServerTimingModule;

use features::audio::{AudioHttpModule, AudioScheduleModule};
use features::authn::AuthnCoreModule;
use features::authz::{AuthzGraphqlModule, AuthzHttpModule};
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        AuthnCoreModule,
        AuthzHttpModule,
        AuthzGraphqlModule,
        OrgsHttpModule,
        OrgsGraphqlModule,
        UsersHttpModule,
        UsersGraphqlModule,
        AudioHttpModule,
        AudioScheduleModule,
        ScheduleModule,
        GraphqlModule::for_root(None),
        HealthModule,
        OpenApiModule::for_root(None),
        OpenTelemetryModule,
        ServerTimingModule,
        HttpModule::for_root(HttpConfig { port: 3002, ..Default::default() }),
    ],
)]
pub struct PlatformApiModule;
