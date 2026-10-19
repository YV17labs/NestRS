//! The store's indicator: one attempt of the boot's proof — a page of the
//! bucket's listing under its prefix — under the crate's name.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_health::IndicatorFuture;

use crate::store::Store;

/// The probe body's key: the crate owning the store.
pub(crate) const NAME: &str = "storage";

/// List the bucket once, read as the boot's proof reads it.
pub(crate) fn listed(container: &Container) -> IndicatorFuture<'_> {
    Box::pin(async move {
        present(container)?.answers().await?;
        Ok(())
    })
}

/// The store `StorageModule` opened, or the refusal naming that import.
pub(crate) fn present(container: &Container) -> anyhow::Result<Arc<Store>> {
    container.get::<Store>().ok_or_else(|| {
        anyhow::anyhow!(
            "StorageHealthModule: the object store was not opened — import `StorageModule`, \
             whose factory opens it"
        )
    })
}
