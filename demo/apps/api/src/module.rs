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
use features::notifications::NotificationsHttpModule;
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::posts::{PostsGraphqlModule, PostsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
        OpenTelemetryModule,
        SeaOrmModule,
        SeaOrmDatabaseModule,
        RedisModule,
        RedisQueueModule,
        HealthModule,
        ServerTimingModule,
        RedisScheduleModule,
        ThrottlerModule,
        OpenApiModule,
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
