use std::any::TypeId;

use nest_rs_core::Container;

use crate::EventBus;

/// Link-time inventory entry submitted by `#[listeners]` for each
/// `#[on_event]`-tagged method. [`crate::EventsModule`] drains the registry at
/// bootstrap and filters by
/// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so a method on a
/// provider not reachable from the app's module tree is warned and skipped
/// (boot `tracing::warn`, target `nest_rs::events`) — never silently dropped, so
/// leftover code doesn't disappear without a trace.
pub struct ListenerMethod {
    /// `module_path!()` of the crate that declared it — read by
    /// [`is_framework_owned`](::nest_rs_core::is_framework_owned) to pick the
    /// report level, and emitted as a field so a skip line names a type the
    /// developer can find.
    pub origin: &'static str,
    /// The listener method's name — the `method` field in the boot wire log.
    pub name: &'static str,
    /// `TypeId` of the host provider, matched against the reachable set to
    /// module-gate this listener.
    pub provider_type_id: fn() -> TypeId,
    /// Position of this method **within its own `#[listeners]` block**. The
    /// registry is link-ordered, which is stable per binary but reshuffles when
    /// the code changes; combined with the provider's rank in
    /// [`ProviderOrder`](::nest_rs_core::ProviderOrder) this restores the order
    /// the developer wrote — providers as they appear in `providers = [...]`,
    /// then methods as they appear in the block.
    pub declaration_index: usize,
    /// Resolves the provider from the assembled container and subscribes a
    /// closure to the bus for the method's event type.
    pub wire: fn(&Container, &EventBus),
}

::nest_rs_core::inventory::collect!(ListenerMethod);
