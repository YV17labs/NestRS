use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::openapi::OpenApiModule;
use nest_rs::opentelemetry::OpenTelemetryModule;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisScheduleModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::server_timing::ServerTimingModule;
use nest_rs::throttler::ThrottlerModule;

use features::audio::{AudioHttpModule, AudioScheduleModule};
use features::audit::AuditHttpModule;
use features::authn::AuthnModule;
use features::authz::{AuthzGraphqlModule, AuthzModule};
use features::notifications::NotificationsHttpModule;
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::posts::{PostsGraphqlModule, PostsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        OpenTelemetryModule,
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        HealthModule,
        ServerTimingModule,
        RedisScheduleModule,
        ThrottlerModule::for_root(None),
        OpenApiModule::for_root(None),
        AuthnModule,
        AuthzModule,
        AuthzGraphqlModule,
        AuditHttpModule,
        OrgsHttpModule,
        OrgsGraphqlModule,
        UsersHttpModule,
        UsersGraphqlModule,
        PostsHttpModule,
        PostsGraphqlModule,
        NotificationsHttpModule,
        AudioHttpModule,
        AudioScheduleModule,
    ],
)]
pub struct ApiModule;
