//! Ownership of the per-namespace [`WsServer<N>`](crate::WsServer) registries:
//! a namespaced gateway submits a link-time [`WsNamespaceEntry`], and
//! [`WsNamespaces`], a provider of [`WsModule`], installs each `WsServer<N>`.
//!
//! [`WsModule`]: crate::WsModule

use std::any::TypeId;

use nest_rs_core::{ContainerBuilder, Discoverable, inventory};

/// One namespaced registry a linked `#[gateway(namespace = N)]` needs.
pub struct WsNamespaceEntry {
    /// `TypeId::of::<WsServer<N>>()`, the container key.
    pub key: fn() -> TypeId,
    /// How the key reads in a boot error — `"WsServer<NotifyNs>"`.
    pub label: &'static str,
    /// Install `WsServer<N>::default()` under [`key`](Self::key).
    pub provide: fn(ContainerBuilder) -> ContainerBuilder,
}

inventory::collect!(WsNamespaceEntry);

/// Installs every linked namespace's [`WsServer<N>`](crate::WsServer), as a
/// provider of [`WsModule`](crate::WsModule).
///
/// Not module-gated: `ReachableProviders` is only known after this register
/// phase, and nothing is mounted or exposed, only an idle registry created.
pub struct WsNamespaces;

impl Discoverable for WsNamespaces {
    /// The keys this provider installs on `WsModule`'s behalf, so the access
    /// graph attributes `WsServer<NotifyNs>` to `WsModule`.
    fn also_provides() -> Vec<(TypeId, &'static str)> {
        inventory::iter::<WsNamespaceEntry>()
            .map(|entry| ((entry.key)(), entry.label))
            .collect()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        inventory::iter::<WsNamespaceEntry>().fold(builder, |builder, entry| {
            // A duplicate submission is reported by the container's duplicate-provider check.
            (entry.provide)(builder)
        })
    }
}
