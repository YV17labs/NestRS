//! The kernel's **wire** step: each registry a module fills from the assembled
//! container is filled once, after the seal and before any transport is built
//! or any lifecycle hook runs.
//!
//! A module attaches its wiring with
//! [`ContainerBuilder::provide_wiring`](crate::ContainerBuilder::provide_wiring);
//! both boot paths drain every one, in registration order, as their last step.
//! A registry the app's code reads is filled here, never by a lifecycle hook:
//! a hook that ran first would read it empty.

use crate::container::Container;
use crate::discovery::Discovery;
use crate::error::WiringFailedError;

/// The function a wiring runs: synchronous, so no I/O slows the boot from here.
pub(crate) type Wire = fn(&Container) -> anyhow::Result<()>;

/// A wiring a module attached with
/// [`ContainerBuilder::provide_wiring`](crate::ContainerBuilder::provide_wiring)
/// — readable through [`Discovery::meta`], built only by the kernel.
pub struct WiringContribution {
    name: &'static str,
    wire: Wire,
}

impl WiringContribution {
    pub(crate) fn new(name: &'static str, wire: Wire) -> Self {
        Self { name, wire }
    }

    /// The registry this wiring fills, as a boot failure names it
    /// (`"nest_rs::events::listeners"`).
    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// Run every attached wiring against the sealed `container`, in registration
/// order; the first that fails ends the boot naming its registry.
pub(crate) fn wire(container: &Container) -> Result<(), WiringFailedError> {
    for contribution in Discovery::new(container).meta::<WiringContribution>() {
        let registry = contribution.meta.name;
        (contribution.meta.wire)(container)
            .map_err(|source| WiringFailedError { registry, source })?;
        tracing::debug!(
            target: crate::target::CONTAINER,
            registry,
            "registry wired",
        );
    }
    Ok(())
}
