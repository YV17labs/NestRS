use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_health::HealthModule;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_seaorm::DatabaseModule;

use crate::chat::ChatModule as ChatFeatureModule;
use crate::notify::NotifyModule;
use features::authn::AuthnModule;
use features::users::UsersWsModule;

#[module(
    imports = [
        ConfigModule::for_root(),
        DatabaseModule::for_root(None),
        AuthnModule,
        HealthModule,
        HttpModule::for_root(HttpConfig { port: 3004, ..Default::default() }),
        ChatFeatureModule,
        NotifyModule,
        UsersWsModule,
    ],
)]
pub struct LiveModule;
