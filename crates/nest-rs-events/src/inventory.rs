use std::any::TypeId;

use nest_rs_core::Container;

use crate::EventBus;

/// Link-time inventory entry submitted by `#[listeners]` for each
/// `#[on_event]`-tagged method; a method on a provider the app's module tree
/// does not reach is warned and skipped at bootstrap.
pub struct ListenerMethod {
    /// `module_path!()` of the crate that declared it.
    pub origin: &'static str,
    /// The listener method's name.
    pub name: &'static str,
    /// `TypeId` of the host provider, matched against the reachable set.
    pub provider_type_id: fn() -> TypeId,
    /// Position of this method within its own `#[listeners]` block; with the
    /// provider's [`ProviderOrder`](::nest_rs_core::ProviderOrder) rank it
    /// restores the written order the link-ordered registry loses.
    pub declaration_index: usize,
    /// Resolves the provider from the assembled container and subscribes a
    /// closure to the bus for the method's event type.
    pub wire: fn(&Container, &EventBus),
}

::nest_rs_core::inventory::collect!(ListenerMethod);
