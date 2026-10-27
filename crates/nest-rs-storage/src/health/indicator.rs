//! The store's indicator: one attempt of the boot's proof — a page of the
//! bucket's listing under its prefix — under the crate's name.

use nest_rs_core::{Container, ContainerBuilder, Module};
use nest_rs_health::{IndicatorEntry, IndicatorFuture, ProbeKind};

use crate::store::Store;

/// The probe body's key: the crate owning the store.
const NAME: &str = "storage";

/// `builder` with the store's indicator on the readiness and startup probes,
/// attached by `M`, the module opening the store — so an app importing
/// `HealthModule` checks it, and liveness never waits on it. A store refusing
/// the listing itself (a policy granting no `s3:ListBucket`) reads up, as it
/// boots, the probe first meeting the refusal filing the boot's `warn`.
pub(crate) fn register<M: Module>(builder: ContainerBuilder) -> ContainerBuilder {
    builder
        .provide_meta(IndicatorEntry::new::<M>(NAME, ProbeKind::Readiness, listed))
        .provide_meta(IndicatorEntry::new::<M>(NAME, ProbeKind::Startup, listed))
}

/// List the bucket once, read as the boot's proof reads it.
fn listed(container: &Container) -> IndicatorFuture<'_> {
    Box::pin(async move {
        let store = container.get::<Store>().ok_or_else(|| {
            anyhow::anyhow!("the store the `storage` indicator lists is not in the container")
        })?;
        store.answers().await?;
        Ok(())
    })
}
