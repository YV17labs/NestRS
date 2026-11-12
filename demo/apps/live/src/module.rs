use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::seaorm::SeaOrmDatabaseModule;

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
)]
pub struct LiveModule;
