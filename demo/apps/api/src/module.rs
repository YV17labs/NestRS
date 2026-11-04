use nest_rs::core::{GlobalInterceptors, module};
use nest_rs::guards::GlobalGuards;
use nest_rs::health::HealthModule;
use nest_rs::openapi::OpenApiModule;
use nest_rs::redis::{RedisModule, RedisQueueModule, RedisScheduleModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::server_timing::ServerTimingModule;
use nest_rs::throttler::ThrottlerModule;

use features::audio::{AudioHttpModule, AudioScheduleModule};
use features::audit::{AuditHttpModule, AuditInterceptor};
use features::authn::AuthnGuard;
use features::authz::AuthzGuard;
use features::notifications::NotificationsHttpModule;
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::posts::{PostsGraphqlModule, PostsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
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
    providers = [
        GlobalGuards<(AuthnGuard, AuthzGuard)>,
        GlobalInterceptors<(AuditInterceptor,)>,
    ],
)]
pub struct ApiModule;
