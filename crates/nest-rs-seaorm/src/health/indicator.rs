//! Pool ping wired through `nest-rs-health`'s indicator registry.
//!
//! Import [`DatabaseHealthModule`] alongside `nest_rs_health::HealthModule` to
//! gate `GET /health/ready` (and `/startup`) on a round-trip to the database.
//! The indicator runs `DatabaseConnection::ping`, so an unreachable DB drops
//! the readiness probe to `503` until the connection comes back.

use std::sync::Arc;

use nest_rs_core::{injectable, module};
use nest_rs_health::indicators;
use sea_orm::DatabaseConnection;

#[injectable]
pub struct DbHealthIndicator {
    #[inject]
    db: Arc<DatabaseConnection>,
}

#[indicators]
impl DbHealthIndicator {
    #[readiness]
    async fn db(&self) -> Result<(), sea_orm::DbErr> {
        self.db.ping().await
    }

    #[startup]
    async fn db_ready(&self) -> Result<(), sea_orm::DbErr> {
        self.db.ping().await
    }
}

#[module(providers = [DbHealthIndicator])]
pub struct DatabaseHealthModule;
