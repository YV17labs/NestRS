use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_health::indicators;
use sea_orm::DatabaseConnection;

/// Health indicator that pings the pool on the readiness and startup probes, so
/// an unreachable database drops those probes to `503`.
#[injectable]
pub struct SeaOrmHealthIndicator {
    #[inject]
    db: Arc<DatabaseConnection>,
}

#[indicators]
impl SeaOrmHealthIndicator {
    #[readiness]
    async fn db(&self) -> Result<(), sea_orm::DbErr> {
        self.db.ping().await
    }

    #[startup]
    async fn db_ready(&self) -> Result<(), sea_orm::DbErr> {
        self.db.ping().await
    }
}
