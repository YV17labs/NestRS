use nest_rs::core::{GlobalInterceptors, module};
use nest_rs::health::HealthModule;
use nest_rs::openapi::OpenApiModule;
use nest_rs::redis::{RedisQueueModule, RedisScheduleModule};
use nest_rs::seaorm::SeaOrmDatabaseModule;
use nest_rs::server_timing::ServerTimingModule;

use features::audio::{AudioHttpModule, AudioScheduleModule};
use features::audit::{AuditHttpModule, AuditInterceptor};
use features::notifications::NotificationsHttpModule;
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::posts::{PostsGraphqlModule, PostsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
        SeaOrmDatabaseModule,
        RedisQueueModule,
        HealthModule,
        ServerTimingModule,
        RedisScheduleModule,
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
    providers = [GlobalInterceptors<(AuditInterceptor,)>],
)]
pub struct ApiModule;
