//! The pool's indicator: a round-trip on the readiness and startup probes,
//! under the crate's own name on both — the probe is the route.

use nest_rs_core::{Container, ContainerBuilder, Module};
use nest_rs_health::{IndicatorEntry, IndicatorFuture, ProbeKind};
use sea_orm::DatabaseConnection;

/// The probe body's key: the crate owning the pool, never a probe-suffixed
/// name (`db_ready`).
const NAME: &str = "seaorm";

/// `builder` with the pool's indicator on the readiness and startup probes,
/// attached by `M`, the module opening the pool — so an app importing
/// `HealthModule` checks it, and liveness never waits on it.
pub(crate) fn register<M: Module>(builder: ContainerBuilder) -> ContainerBuilder {
    builder
        .provide_meta(IndicatorEntry::new::<M>(NAME, ProbeKind::Readiness, ping))
        .provide_meta(IndicatorEntry::new::<M>(NAME, ProbeKind::Startup, ping))
}

/// Ping the pool [`SeaOrmModule`](crate::SeaOrmModule) opened, so an
/// unreachable database reads down.
fn ping(container: &Container) -> IndicatorFuture<'_> {
    Box::pin(async move {
        let db = container.get::<DatabaseConnection>().ok_or_else(|| {
            anyhow::anyhow!("the pool the `seaorm` indicator pings is not in the container")
        })?;
        db.ping().await?;
        Ok(())
    })
}
