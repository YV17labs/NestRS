use std::any::TypeId;

use nest_rs_core::Container;

use crate::EventBus;

/// Link-time inventory entry submitted by `#[listeners]` for each
/// `#[on_event]`-tagged method. [`crate::EventsModule`] drains the registry at
/// bootstrap and filters by
/// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so a method on a
/// provider not reachable from the app's module tree is silently skipped.
pub struct ListenerMethod {
    pub name: &'static str,
    pub provider_type_id: fn() -> TypeId,
    /// Resolves the provider from the assembled container and subscribes a
    /// closure to the bus for the method's event type.
    pub wire: fn(&Container, &EventBus),
}

::nest_rs_core::inventory::collect!(ListenerMethod);
