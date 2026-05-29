//! Discovery metadata attached by `#[event_handler]`.

use nestrs_core::Container;

use crate::EventBus;

/// Discovery metadata attached by `#[event_handler]`. [`EventModule`](crate::EventModule)'s
/// bootstrap hook reads these via `DiscoveryService::meta::<EventHandlerMeta>()` from
/// the assembled container and runs each [`wire`](EventHandlerMeta::wire) to build the
/// handler and subscribe it to the bus. Fields are `pub` only so generated code can
/// build it.
pub struct EventHandlerMeta {
    pub name: &'static str,
    /// Build the handler from the container and subscribe it to the bus.
    pub wire: fn(&Container, &EventBus),
}
