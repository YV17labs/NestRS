use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::http::{HttpConfig, HttpModule};
use nest_rs::seaorm::{DatabaseHealthModule, DatabaseModule};

use crate::chat::ChatModule as ChatFeatureModule;
use crate::notify::NotifyModule;
use features::authn::AuthnModule;
use features::users::UsersWsModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        DatabaseHealthModule,
        AuthnModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3004, ..Default::default() }),
        ChatFeatureModule,
        NotifyModule,
        UsersWsModule,
    ],
)]
pub struct LiveModule;
