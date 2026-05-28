use std::any::TypeId;

use crate::container::ContainerBuilder;

/// Anything a `#[module]` can pull in via `providers = [...]`.
///
/// The macros that decorate a struct (`#[injectable]`, `#[interceptor]`, and
/// future `#[cron_job]`/`#[event_handler]`/`#[mcp_tool]`/…) — together with
/// the `#[routes]` macro on a controller's `impl` block — emit a single
/// `impl Discoverable for Self` per type. The implementation either:
///
/// - registers the value as a provider (`provide` / `provide_dyn`), or
/// - attaches a piece of discovery metadata to the type
///   ([`ContainerBuilder::attach_meta`]), or both.
///
/// `#[module]` then loops over its `providers = [...]` list and calls
/// `<T as Discoverable>::register(builder)` uniformly — it knows nothing
/// about HTTP, MCP, GraphQL, or any future surface.
pub trait Discoverable {
    /// The provider types that must already be registered before
    /// [`register`](Discoverable::register) can build this one. `#[module]`
    /// reads this to register providers in dependency order, so the
    /// `providers = [...]` list can be written in any order.
    ///
    /// The default — no dependencies — fits anything that resolves its
    /// dependencies lazily rather than at registration time: a controller
    /// builds at mount time, a resolver at schema-build time, so neither
    /// needs its dependencies present when `register` runs. Providers built
    /// eagerly (`#[injectable]`, `#[interceptor]`) override this to list the
    /// `TypeId` of each `#[inject]` dependency.
    fn dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// The `TypeId` of each `#[inject]` dependency this provider pulls from the
    /// container — *whenever* it is built — for the module access-graph check
    /// `#[module]` records it per provider so the
    /// boot-time pass can verify the dependency is reachable through the
    /// module's imports (or is global infrastructure).
    ///
    /// Distinct from [`dependencies`](Discoverable::dependencies): that gates
    /// *registration ordering* and so is empty for a provider built later from
    /// the fully-assembled container (a controller, MCP tool, cron job,
    /// processor), which must not block the register-phase fixpoint. `injected`
    /// reports the same `#[inject]` fields regardless of build timing, so the
    /// access contract governs transport-built logic too. The default — none —
    /// fits a provider with no injected dependencies; the decorator macros
    /// override it from the `#[inject]` fields.
    fn injected() -> Vec<TypeId> {
        Vec::new()
    }

    /// A human-readable label for each [`dependencies`](Discoverable::dependencies)
    /// entry, in the same order, so the `#[module]` boot-time fixpoint can name a
    /// missing dependency rather than only the provider that needs it. The default
    /// — none — leaves the diagnostic to fall back to the provider name; the
    /// eager-provider decorators (`#[injectable]`, `#[interceptor]`) override it.
    fn dependency_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// The `TypeId` of each `#[inject] Option<Arc<…>>` *optional* dependency (the
    /// `@Optional` analog). Unlike [`dependencies`](Discoverable::dependencies),
    /// the register-phase fixpoint does not require these to be present — a missing
    /// one resolves to `None` — but it does use them to order the provider after an
    /// optional dependency the same module supplies, preserving order-independence.
    fn optional_dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}
