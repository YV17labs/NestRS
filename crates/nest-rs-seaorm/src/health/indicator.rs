//! Pool ping wired through `nest-rs-health`'s indicator registry.
//!
//! [`DbHealthIndicator`] runs `DatabaseConnection::ping` on the readiness and
//! startup probes, so an unreachable DB drops those probes to `503` until the
//! connection comes back. [`DatabaseHealthModule`](super::DatabaseHealthModule)
//! is the import seam that registers it.

use std::sync::Arc;

use nest_rs_core::injectable;
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
