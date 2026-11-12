//! The read side of discovery — [`Discovery`], over the container's metadata
//! index.

use std::any::{Any, TypeId};
use std::sync::Arc;

use crate::container::Container;

/// Read-side facade over the container's metadata index, for transports and
/// scanners (OpenAPI, cron, MCP, …) to find the surfaces they mount.
pub struct Discovery<'a> {
    container: &'a Container,
}

impl<'a> Discovery<'a> {
    /// Borrow a container to read discovery metadata from.
    pub fn new(container: &'a Container) -> Self {
        Self { container }
    }

    /// Every piece of metadata of type `M` in registration order.
    pub fn meta<M: Any + Send + Sync>(&self) -> Vec<Discovered<M>> {
        self.container
            .metadata_entries(TypeId::of::<M>())
            .map_or_default(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        entry
                            .meta
                            .clone()
                            .downcast::<M>()
                            .ok()
                            .map(|meta| Discovered {
                                meta,
                                host_type_id: entry.host_type_id,
                            })
                    })
                    .collect()
            })
    }
}

/// A discovered piece of metadata, paired with the host provider's `TypeId`
/// when host-bound. Scanners invoke the live provider through closures
/// embedded in `meta`.
#[non_exhaustive]
pub struct Discovered<M> {
    /// The metadata payload the provider submitted at registration.
    pub meta: Arc<M>,
    /// `TypeId` of the host provider when the metadata is host-bound (a scanner
    /// resolves the live provider through it); `None` for free-standing metadata.
    pub host_type_id: Option<TypeId>,
}
