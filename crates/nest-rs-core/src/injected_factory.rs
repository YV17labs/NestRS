//! [`InjectedFactory`] — a factory the boot queues for a type a reachable
//! provider injects, so the injection is the whole declaration.

use std::any::TypeId;
use std::collections::HashSet;

use crate::access::__private::ModuleDescriptor;
use crate::container::{Container, ContainerBuilder};

/// The factory of a type that loads itself once a provider of a reached
/// module requires it (`#[inject] Arc<T>`, never an `Option`), submitted to
/// the link-time registry by the decorator that declares the type —
/// `#[config]` files every config so.
///
/// Loading one mounts and exposes nothing: what it builds is global
/// infrastructure, like any factory's output, and the requirement that
/// queues it is a provider's of a module the app imports, so the load is
/// module-gated by construction. An import that already queued the type — a
/// `for_root` pinning it, a module queuing it from its own `collect` — keeps
/// it; this one is queued only where none did. A seed of the type is held to
/// `check`, what its factory's value would pass, in a container built by hand
/// too, where no factory runs.
pub struct InjectedFactory {
    key: fn() -> TypeId,
    queue: fn(ContainerBuilder) -> ContainerBuilder,
    check: fn(&Container) -> anyhow::Result<()>,
}

impl InjectedFactory {
    /// The entry for the type `key` names, queued through `queue`, a seed of
    /// it held to `check`.
    pub const fn new(
        key: fn() -> TypeId,
        queue: fn(ContainerBuilder) -> ContainerBuilder,
        check: fn(&Container) -> anyhow::Result<()>,
    ) -> Self {
        Self { key, queue, check }
    }

    /// The type this entry loads.
    pub(crate) fn key(&self) -> TypeId {
        (self.key)()
    }

    /// What a seed of the type passes before anything reads it.
    pub(crate) fn check(&self) -> fn(&Container) -> anyhow::Result<()> {
        self.check
    }
}

inventory::collect!(InjectedFactory);

/// Queue the factory of every registered type a provider of `reached`
/// requires and no import queued, as the collect phase ends.
pub(crate) fn queue_injected(
    mut builder: ContainerBuilder,
    reached: &[&ModuleDescriptor],
) -> ContainerBuilder {
    let mut required: HashSet<TypeId> = reached
        .iter()
        .flat_map(|module| module.providers)
        .flat_map(|provider| (provider.injects)())
        .collect();
    for entry in inventory::iter::<InjectedFactory>() {
        let key = entry.key();
        if required.remove(&key) && !builder.queues(key) {
            builder = (entry.queue)(builder);
        }
    }
    builder
}
