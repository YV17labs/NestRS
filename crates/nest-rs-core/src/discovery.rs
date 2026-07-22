//! The read side of discovery — [`DiscoveryService`], the transport-agnostic
//! facade over the container's metadata index that transports and applicative
//! scanners (OpenAPI, cron, MCP, …) query to find the surfaces they mount.

use std::any::{Any, TypeId};
use std::sync::Arc;

use crate::container::Container;

/// Read-side facade over the container's metadata index, used by transports
/// and applicative scanners (OpenAPI, cron, MCP, …) without coupling to a
/// specific transport.
pub struct DiscoveryService<'a> {
    container: &'a Container,
}

impl<'a> DiscoveryService<'a> {
    /// Borrow a container to read discovery metadata from.
    pub fn new(container: &'a Container) -> Self {
        Self { container }
    }

    /// Every piece of metadata of type `M` in registration order.
    pub fn meta<M: Any + Send + Sync>(&self) -> Vec<Discovered<M>> {
        self.container
            .metadata_entries(TypeId::of::<M>())
            .map(|entries| {
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
                                provider_type_id: entry.provider_type_id,
                            })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A discovered piece of metadata, paired with the host provider's `TypeId`
/// when host-bound. Scanners invoke the live provider through closures
/// embedded in `meta`.
pub struct Discovered<M> {
    /// The metadata payload the provider submitted at registration.
    pub meta: Arc<M>,
    /// `TypeId` of the host provider when the metadata is host-bound (a scanner
    /// resolves the live provider through it); `None` for free-standing metadata.
    pub provider_type_id: Option<TypeId>,
}
