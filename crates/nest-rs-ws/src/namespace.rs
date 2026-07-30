//! Ownership of the per-namespace [`WsServer<N>`] registries.
//!
//! [`WsServer<Global>`] is a plain provider of [`WsModule`], so a service that
//! injects it either imports `WsModule` or gets a boot error naming it. A
//! namespaced registry used to work differently: `#[gateway(namespace = N)]`
//! installed its own `WsServer<N>` from `Discoverable::register`, which had two
//! consequences the docs promised the opposite of.
//!
//! - **The access graph could not see it.** No module claimed the key, so the
//!   graph's escape hatch for imperatively-registered types waved every consumer
//!   through. The app booted, mounted its routes, and *then* panicked at first
//!   resolution — naming the controller that happened to be built first rather
//!   than the service whose dependency was missing.
//! - **It was order-sensitive.** A module registers its imports before its own
//!   providers, so any consumer living in a module the gateway imports was
//!   constructed before the gateway existed. Correct wiring depended on where in
//!   the tree the gateway sat, which is not something a developer can be asked to
//!   reason about.
//!
//! [`WsModule`] now owns **every** registry, namespaced or not. A namespaced
//! gateway submits a link-time [`WsNamespaceEntry`]; [`WsNamespaces`] — a
//! provider of `WsModule` — drains that inventory and installs each
//! `WsServer<N>`. One rule for both cases: the registry comes from `WsModule`,
//! the import is what the graph checks, and a missing one is the same named boot
//! error either way. Order stops mattering because the owner is a leaf module
//! both the gateway's side and the pushing service's side can import.
//!
//! [`WsModule`]: crate::WsModule
//! [`WsServer<Global>`]: crate::WsServer

use std::any::TypeId;

use nest_rs_core::{ContainerBuilder, Discoverable, inventory};

/// One namespaced registry a linked `#[gateway(namespace = N)]` needs.
///
/// **Internal ABI** — submitted by the `#[gateway]` macro, lockstep with this
/// crate; do not hand-construct.
#[doc(hidden)]
pub struct WsNamespaceEntry {
    /// `TypeId::of::<WsServer<N>>()`, the container key.
    pub key: fn() -> TypeId,
    /// How the key reads in a boot error — `"WsServer<NotifyNs>"`.
    pub label: &'static str,
    /// Install `WsServer<N>::default()` under [`key`](Self::key).
    pub provide: fn(ContainerBuilder) -> ContainerBuilder,
}

inventory::collect!(WsNamespaceEntry);

/// Installs every linked namespace's [`WsServer<N>`](crate::WsServer). A
/// provider of [`WsModule`](crate::WsModule), so importing that one module is
/// what makes a namespaced registry injectable — see the module docs.
///
/// Not module-gated by `ReachableProviders`: that set is only known after the
/// register phase this runs in, and an idle registry for a namespace whose
/// gateway is not mounted costs two empty maps. Deterministic beats clever here
/// — nothing is *discovered*, so the "discovery is module-gated" rule is not in
/// play; only a singleton is created.
pub struct WsNamespaces;

impl Discoverable for WsNamespaces {
    /// The keys this provider installs on `WsModule`'s behalf, so the access
    /// graph attributes `WsServer<NotifyNs>` to `WsModule` and a consumer that
    /// forgot the import gets the named error rather than a late panic.
    fn also_provides() -> Vec<(TypeId, &'static str)> {
        inventory::iter::<WsNamespaceEntry>()
            .map(|entry| ((entry.key)(), entry.label))
            .collect()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        inventory::iter::<WsNamespaceEntry>().fold(builder, |builder, entry| {
            // `provide` is idempotent in effect (the same key, a fresh empty
            // registry), but a duplicate submission would still be a wiring bug
            // worth not silently overwriting — the container's own
            // duplicate-provider check reports it.
            (entry.provide)(builder)
        })
    }
}
