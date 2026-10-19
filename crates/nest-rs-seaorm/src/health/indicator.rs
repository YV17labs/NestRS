//! The pool's indicator: a round-trip on the readiness and startup probes,
//! under the crate's own name on both — the probe is the route.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_health::IndicatorFuture;
use sea_orm::DatabaseConnection;

use crate::module::SUBSTRATE_REMEDY;

/// The probe body's key: the crate owning the pool, never a probe-suffixed
/// name (`db_ready`).
pub(crate) const NAME: &str = "seaorm";

/// Ping the pool [`SeaOrmModule`](crate::SeaOrmModule) opened, so an
/// unreachable database reads down.
pub(crate) fn ping(container: &Container) -> IndicatorFuture<'_> {
    Box::pin(async move {
        present(container)?.ping().await?;
        Ok(())
    })
}

/// The pool, or the refusal naming the import that opens it.
pub(crate) fn present(container: &Container) -> anyhow::Result<Arc<DatabaseConnection>> {
    container.get::<DatabaseConnection>().ok_or_else(|| {
        anyhow::anyhow!(
            "SeaOrmHealthModule: no `DatabaseConnection` in the container — {SUBSTRATE_REMEDY}"
        )
    })
}
