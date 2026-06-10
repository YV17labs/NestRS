use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_graphql::GraphqlModule;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_openapi::OpenApiModule;
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_redis::QueueModule;
use nest_rs_schedule::ScheduleModule;
use nest_rs_seaorm::{DatabaseHealthModule, DatabaseModule};
use nest_rs_server_timing::ServerTimingModule;

use features::audio::{AudioHttpModule, AudioScheduleModule};
use features::authn::AuthnModule;
use features::authz::{AuthzGraphqlModule, AuthzHttpModule};
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::posts::PostsHttpModule;
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        DatabaseModule::for_root(None),
        DatabaseHealthModule,
        QueueModule::for_root(None),
        HealthModule,
        ServerTimingModule,
        ScheduleModule,
        HttpModule::for_root(HttpConfig { port: 3002, ..Default::default() }),
        GraphqlModule::for_root(None),
        OpenApiModule::for_root(None),
        AuthnModule,
        AuthzHttpModule,
        AuthzGraphqlModule,
        OrgsHttpModule,
        OrgsGraphqlModule,
        UsersHttpModule,
        UsersGraphqlModule,
        PostsHttpModule,
        AudioHttpModule,
        AudioScheduleModule,
    ],
)]
pub struct ApiModule;
