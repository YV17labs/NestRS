use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};

use features::authn::AuthnModule;
use features::chat::ChatWsModule;
use features::notifications::NotificationsWsModule;
use features::users::UsersWsModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        AuthnModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3004, ..Default::default() }),
        ChatWsModule,
        NotificationsWsModule,
        UsersWsModule,
    ],
)]
pub struct LiveModule;
