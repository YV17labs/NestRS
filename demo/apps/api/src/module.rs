use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::graphql::GraphqlModule;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::openapi::OpenApiModule;
use nest_rs::redis::{RedisModule, RedisQueueModule};
use nest_rs::schedule::ScheduleModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule, SeaOrmModule};
use nest_rs::server_timing::ServerTimingModule;
use nest_rs::throttler::ThrottlerModule;

use features::audio::{AudioHttpModule, AudioScheduleModule};
use features::authn::AuthnModule;
use features::authz::{AuthzGraphqlModule, AuthzModule};
use features::notifications::{NotificationsEventsModule, NotificationsHttpModule};
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::posts::{PostsGraphqlModule, PostsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        SeaOrmHealthModule,
        RedisModule::for_root(None),
        RedisQueueModule,
        HealthModule,
        ServerTimingModule,
        ScheduleModule,
        HttpModule::for_root(HttpConfig {
            port: 3002,
            compression: true,
            ..Default::default()
        }),
        ThrottlerModule::for_root(None),
        GraphqlModule::for_root(None),
        OpenApiModule::for_root(None),
        AuthnModule,
        AuthzModule,
        AuthzGraphqlModule,
        OrgsHttpModule,
        OrgsGraphqlModule,
        UsersHttpModule,
        UsersGraphqlModule,
        PostsHttpModule,
        PostsGraphqlModule,
        NotificationsEventsModule,
        NotificationsHttpModule,
        AudioHttpModule,
        AudioScheduleModule,
    ],
)]
pub struct ApiModule;
