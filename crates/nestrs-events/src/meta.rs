use nestrs_core::Container;

use crate::EventBus;

pub struct EventHandlerMeta {
    pub name: &'static str,
    /// Build the handler from the container and subscribe it to the bus.
    pub wire: fn(&Container, &EventBus),
}
