use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::graphql::GraphqlModule;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::openapi::OpenApiModule;
use nest_rs::redis::RedisQueueModule;
use nest_rs::schedule::ScheduleModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule};
use nest_rs::server_timing::ServerTimingModule;
use nest_rs::throttler::ThrottlerModule;

use features::app_authn::AppAuthnModule;
use features::app_authz::{AppAuthzGraphqlModule, AppAuthzHttpModule};
use features::audio::{AudioHttpModule, AudioScheduleModule};
use features::notifications::{NotificationsEventsModule, NotificationsHttpModule};
use features::orgs::{OrgsGraphqlModule, OrgsHttpModule};
use features::posts::{PostsGraphqlModule, PostsHttpModule};
use features::users::{UsersGraphqlModule, UsersHttpModule};

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmDatabaseModule::for_root(None),
        SeaOrmHealthModule,
        RedisQueueModule::for_root(None),
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
        AppAuthnModule,
        AppAuthzHttpModule,
        AppAuthzGraphqlModule,
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
