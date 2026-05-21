use std::any::{Any, TypeId};
use std::sync::Arc;

use crate::container::Container;

/// Read-side façade over the container's metadata index. Transports
/// (`HttpTransport`, future MCP/gRPC ones) and applicative scanners (a cron
/// scheduler, an event bus, an OpenAPI generator, …) use this to find what
/// they care about without coupling to a specific transport.
///
/// Each piece of metadata is attached at registration time via
/// [`crate::ContainerBuilder::attach_meta`] (host-bound) or
/// [`crate::ContainerBuilder::provide_meta`] (free-standing). The macros that
/// describe a provider — `#[routes]` for an HTTP controller, future
/// `#[cron_job]`, `#[mcp_tool]`, … — emit the `attach_meta` call so the
/// developer never touches it by hand.
pub struct DiscoveryService<'a> {
    container: &'a Container,
}

impl<'a> DiscoveryService<'a> {
    pub fn new(container: &'a Container) -> Self {
        Self { container }
    }

    /// Every piece of metadata of type `M` registered in the container, in
    /// registration order.
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

/// A discovered piece of metadata, paired with the `TypeId` of the provider
/// it describes (when host-bound). Scanners that need to invoke methods on
/// the live provider use the closures embedded inside `meta` — the macros
/// generate them with the concrete type in scope.
pub struct Discovered<M> {
    pub meta: Arc<M>,
    pub provider_type_id: Option<TypeId>,
}
