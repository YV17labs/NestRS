//! The read side of discovery — [`DiscoveryService`], the transport-agnostic
//! facade over the container's metadata index that transports and applicative
//! scanners (OpenAPI, cron, MCP, …) query to find the surfaces they mount.

use std::any::{Any, TypeId};
use std::sync::Arc;

use crate::access::{ModuleDescriptor, ReachableProviders, ResolverDescriptor};
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

    /// Read-only snapshot of the access graph as it was validated at boot.
    ///
    /// `modules` and `resolvers` are link-time `inventory` registries — the
    /// snapshot recollects them on each call (cheap; both are immutable past
    /// boot). `reachable` is the `ReachableProviders` set seeded by
    /// [`App::run`](crate::App::run); `None` on a hand-built container that
    /// has no module gating active.
    pub fn graph(&self) -> AccessGraphSnapshot {
        AccessGraphSnapshot {
            modules: inventory::iter::<ModuleDescriptor>().collect(),
            resolvers: inventory::iter::<ResolverDescriptor>().collect(),
            reachable: self.container.get::<ReachableProviders>(),
        }
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

/// Snapshot of the validated module/provider graph for read-only consumers
/// (health endpoints, devtools, runtime introspection). Returned by
/// [`DiscoveryService::graph`].
///
/// All fields are public — callers query directly. `reachable.is_none()`
/// means "no module gating active" (hand-built container), distinct from
/// "module-gated and this provider isn't reachable".
pub struct AccessGraphSnapshot {
    /// Every module linked into the binary, in `inventory` order.
    pub modules: Vec<&'static ModuleDescriptor>,
    /// Every `#[resolver]` linked into the binary, in `inventory` order.
    pub resolvers: Vec<&'static ResolverDescriptor>,
    /// Provider keys reachable from the running app's root module, or `None`
    /// when no [`ReachableProviders`] was seeded (hand-built container ⇒ no
    /// gating; every provider freely resolvable).
    pub reachable: Option<Arc<ReachableProviders>>,
}
