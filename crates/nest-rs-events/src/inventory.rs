use std::any::TypeId;

use nest_rs_core::Container;

use crate::EventBus;

/// Link-time inventory entry submitted by `#[listeners]` for each
/// `#[on_event]`-tagged method; a method on a provider the app's module tree
/// does not reach is warned and skipped when the boot wires the listeners.
pub struct ListenerMethod {
    origin: &'static str,
    name: &'static str,
    provider_type_id: fn() -> TypeId,
    declaration_index: usize,
    wire: fn(&Container, &EventBus) -> anyhow::Result<()>,
}

impl ListenerMethod {
    /// `module_path!()` of the crate that declared it, which picks the level a
    /// skipped listener is reported at.
    pub(crate) const fn origin(&self) -> &'static str {
        self.origin
    }

    /// `Provider::method`, as the boot lines name the listener.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// `TypeId` of the host provider, matched against the reachable set.
    pub(crate) fn provider_type_id(&self) -> TypeId {
        (self.provider_type_id)()
    }

    /// Position of this method within its own `#[listeners]` block; with the
    /// provider's [`ProviderOrder`](::nest_rs_core::ProviderOrder) rank it
    /// restores the written order the link-ordered registry loses.
    pub(crate) const fn declaration_index(&self) -> usize {
        self.declaration_index
    }

    /// Resolve the provider from the assembled container and subscribe a
    /// closure to the bus for the method's event type, around the pipeline
    /// its site composes; a layer the site binds and nothing provides refuses
    /// it, and the boot with it.
    pub(crate) fn wire(&self, container: &Container, bus: &EventBus) -> anyhow::Result<()> {
        (self.wire)(container, bus)
    }
}

/// The entry `#[listeners]` submits for one `#[on_event]` method; a
/// hand-built entry is a test's, never an app's.
pub const fn listener_method(
    origin: &'static str,
    name: &'static str,
    provider_type_id: fn() -> TypeId,
    declaration_index: usize,
    wire: fn(&Container, &EventBus) -> anyhow::Result<()>,
) -> ListenerMethod {
    ListenerMethod {
        origin,
        name,
        provider_type_id,
        declaration_index,
        wire,
    }
}

::nest_rs_core::inventory::collect!(ListenerMethod);
