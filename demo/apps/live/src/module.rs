use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule, SeaOrmModule};

use crate::chat::ChatModule;
use crate::notify::NotifyModule;
use features::authn::AuthnModule;
use features::users::UsersWsModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        SeaOrmHealthModule,
        AuthnModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3004, ..Default::default() }),
        ChatModule,
        NotifyModule,
        UsersWsModule,
    ],
)]
pub struct LiveModule;
