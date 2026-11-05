//! [`HostRuntime`] — the modules an edge's host runs on, which its decorator
//! brings: the `#[module]` listing the host imports each of them bare, so the
//! app importing an adapter serves it and reads its edge's config.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::container::ContainerBuilder;
use crate::module::Module;

/// One module an edge's hosts run on, as the edge's crate declares it.
pub struct RuntimeModule {
    module: fn() -> TypeId,
    import: fn(ContainerBuilder) -> ContainerBuilder,
}

impl RuntimeModule {
    /// The module `M`, imported bare.
    pub const fn of<M: Module>() -> Self {
        Self {
            module: TypeId::of::<M>,
            import: ContainerBuilder::import::<M>,
        }
    }
}

/// What one host runs on, submitted by the decorator that makes a provider an
/// edge's host, so what is linked and listed by no module the app reaches
/// brings nothing.
pub struct HostRuntime {
    /// The host provider's type.
    pub host: fn() -> TypeId,
    /// The modules its edge's crate declares.
    pub modules: &'static [RuntimeModule],
}

inventory::collect!(HostRuntime);

/// Every entry by host, read once: the access graph walks a module's imports
/// once per module, and the link-time registry never changes.
static BY_HOST: LazyLock<HashMap<TypeId, Vec<&'static RuntimeModule>>> = LazyLock::new(|| {
    let mut by_host: HashMap<TypeId, Vec<&'static RuntimeModule>> = HashMap::new();
    for entry in inventory::iter::<HostRuntime>() {
        by_host
            .entry((entry.host)())
            .or_default()
            .extend(entry.modules);
    }
    by_host
});

/// Import what the provider `host` runs on, in the phase `builder` is in;
/// emitted by `#[module]` for each provider it lists, in both phases.
pub fn import_host_runtime(mut builder: ContainerBuilder, host: TypeId) -> ContainerBuilder {
    for module in BY_HOST.get(&host).into_iter().flatten() {
        builder = (module.import)(builder);
    }
    builder
}

/// The modules the provider `host` runs on, for the access graph's walks.
pub(crate) fn runtime_modules(host: TypeId) -> impl Iterator<Item = TypeId> {
    BY_HOST
        .get(&host)
        .into_iter()
        .flatten()
        .map(|module| (module.module)())
}
