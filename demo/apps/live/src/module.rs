use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::opentelemetry::OpenTelemetryModule;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};

use features::chat::ChatWsModule;
use features::notifications::NotificationsWsModule;
use features::users::UsersWsModule;

#[module(
    imports = [
        OpenTelemetryModule,
        SeaOrmModule,
        SeaOrmDatabaseModule,
        HealthModule,
        ChatWsModule,
        NotificationsWsModule,
        UsersWsModule,
    ],
)]
pub struct LiveModule;
