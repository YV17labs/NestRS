use std::any::TypeId;

use crate::container::{ContainerBuilder, KeyedDependency};

/// Anything a `#[module]` can pull in via `providers = [...]`.
///
/// Decorator macros (`#[injectable]`, `#[interceptor]`, `#[cron_job]`,
/// `#[mcp]`, `#[routes]`, …) emit a single `impl Discoverable for Self` that
/// either registers a provider or attaches discovery metadata.
pub trait Discoverable {
    /// Provider types that must already be registered before
    /// [`register`](Discoverable::register) can build this one — read by
    /// `#[module]` to order registration. Empty for providers built lazily
    /// (controllers, resolvers) so they do not block the register-phase
    /// fixpoint.
    fn dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject]` dependency, recorded for the access-graph
    /// check. Reported regardless of build timing, so the contract governs
    /// transport-built logic too.
    fn injected() -> Vec<TypeId> {
        Vec::new()
    }

    /// Human-readable label for each [`injected`](Discoverable::injected)
    /// entry, in the same order, so the access graph can name a dependency no
    /// module provides — a lazily-built provider's missing dependency is a clean
    /// boot error naming both the provider and the dependency, not a
    /// `get(...).expect(...)` panic at first resolution. May be shorter than
    /// `injected()` (a provider that does not emit names falls back to a
    /// placeholder); never longer.
    fn injected_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// [`ProviderKey`] of each **keyed** `#[inject(key = "…")]` dependency,
    /// recorded for the access-graph keyed check. Kept apart from
    /// [`injected`](Discoverable::injected) — a keyed dependency is validated
    /// against the global keyed set (seeds + factory outputs), and its boot
    /// error names both the type and the key. Empty for providers with no keyed
    /// dependency (the default).
    fn injected_keyed() -> Vec<KeyedDependency> {
        Vec::new()
    }

    /// Human-readable label for each [`dependencies`](Discoverable::dependencies)
    /// entry, in the same order, so the boot-time fixpoint can name a missing
    /// dependency.
    fn dependency_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject] Option<Arc<…>>` optional dependency.
    /// Not required by the register-phase fixpoint, but
    /// used to order the provider after an optional dependency the same module
    /// supplies.
    fn optional_dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}
