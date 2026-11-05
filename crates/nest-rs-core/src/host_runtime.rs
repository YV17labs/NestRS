//! [`HostRuntime`] — the modules an edge's host runs on, which its decorator
//! brings: the `#[module]` listing the host imports each of them bare, so the
//! app importing an adapter serves it and reads its edge's config.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::access::Composition;
use crate::container::{Container, ContainerBuilder};
use crate::module::Module;
use crate::type_name::shorten;

/// One module an edge's hosts run on, as the edge's crate declares it.
pub struct RuntimeModule {
    module: fn() -> TypeId,
    name: fn() -> &'static str,
    import: fn(ContainerBuilder) -> ContainerBuilder,
}

impl RuntimeModule {
    /// The module `M`, imported bare.
    pub const fn of<M: Module>() -> Self {
        Self {
            module: TypeId::of::<M>,
            name: std::any::type_name::<M>,
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
    /// That type's name, shortened where a line gives it.
    pub name: fn() -> &'static str,
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

/// Import what each provider of `listed` runs on, in the phase `builder` is in;
/// emitted by `#[module]` over the providers it lists, in both phases.
pub fn import_host_runtime(mut builder: ContainerBuilder, listed: &[TypeId]) -> ContainerBuilder {
    for module in listed.iter().filter_map(|host| BY_HOST.get(host)).flatten() {
        builder = (module.import)(builder);
    }
    builder
}

/// The modules the provider `host` runs on, for the access graph's walks.
pub(crate) fn runtime_modules(host: TypeId) -> impl DoubleEndedIterator<Item = TypeId> {
    BY_HOST
        .get(&host)
        .into_iter()
        .flatten()
        .map(|module| (module.module)())
}

/// File one `warn` per host the app `container` was booted for holds with none
/// of the modules it runs on imported: seeded, or registered by a hand-written
/// module, so no `#[module]` listing brought them, and no edge will dispatch it.
pub(crate) fn report_unserved(container: &Container) {
    let Some(composition) = container.get::<Composition>() else {
        return;
    };
    let mut hosts: Vec<&'static HostRuntime> = inventory::iter::<HostRuntime>()
        .filter(|entry| container.holds((entry.host)()))
        .filter(|entry| !runtime_modules((entry.host)()).any(|module| composition.imports(module)))
        .collect();
    hosts.sort_unstable_by_key(|entry| ((entry.name)(), (entry.host)()));
    hosts.dedup_by_key(|entry| (entry.host)());
    for entry in hosts {
        let import: Vec<String> = entry
            .modules
            .iter()
            .map(|module| shorten((module.name)()))
            .collect();
        tracing::warn!(
            target: crate::target::APP,
            host = shorten((entry.name)()),
            import = import.join(", "),
            "host not served: its edge's transport is not attached",
        );
    }
}
