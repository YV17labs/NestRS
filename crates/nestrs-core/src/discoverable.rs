use std::any::TypeId;

use crate::container::ContainerBuilder;

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

    /// Human-readable label for each [`dependencies`](Discoverable::dependencies)
    /// entry, in the same order, so the boot-time fixpoint can name a missing
    /// dependency.
    fn dependency_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject] Option<Arc<…>>` optional dependency (the
    /// `@Optional` analog). Not required by the register-phase fixpoint, but
    /// used to order the provider after an optional dependency the same module
    /// supplies.
    fn optional_dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}
