use nest_rs::core::module;
use nest_rs::guards::GlobalGuards;
use nest_rs::health::HealthModule;
use nest_rs::seaorm::SeaOrmDatabaseModule;

use features::authn::AuthnGuard;
use features::chat::ChatWsModule;
use features::notifications::NotificationsWsModule;
use features::users::UsersWsModule;

#[module(
    imports = [
        SeaOrmDatabaseModule,
        HealthModule,
        ChatWsModule,
        NotificationsWsModule,
        UsersWsModule,
    ],
    providers = [GlobalGuards<(AuthnGuard,)>],
)]
pub struct LiveModule;
