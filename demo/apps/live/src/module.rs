use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmHealthModule};

use crate::chat::ChatModule as ChatFeatureModule;
use crate::notify::NotifyModule;
use features::app_authn::AppAuthnModule;
use features::users::UsersWsModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        SeaOrmDatabaseModule::for_root(None),
        SeaOrmHealthModule,
        AppAuthnModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3004, ..Default::default() }),
        ChatFeatureModule,
        NotifyModule,
        UsersWsModule,
    ],
)]
pub struct LiveModule;
