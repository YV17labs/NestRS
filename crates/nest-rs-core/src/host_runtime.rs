//! [`HostRuntime`] — the modules an edge's host runs on, which its decorator
//! brings: the `#[module]` listing the host imports each of them bare, so the
//! app importing an adapter serves it and reads its edge's config. A host no
//! `#[module]` lists brings nothing, and [`App::run`](crate::App::run) says so.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// How an edge finds its hosts, which decides what an instance the container
/// holds under a host's type is to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostMount {
    /// It mounts what the host's `Discoverable::register` attached and builds
    /// its own instance from that, so an instance held is no host of its.
    Attached,
    /// It reads the link-time registry against what the app reaches, as a seeded
    /// instance is.
    Linked,
}

/// What one host runs on, submitted by the decorator that makes a provider an
/// edge's host, so what is linked and listed by no module the app reaches
/// brings nothing.
pub struct HostRuntime {
    /// The host provider's type.
    pub host: fn() -> TypeId,
    /// That type's name, shortened where a line gives it.
    pub name: fn() -> &'static str,
    /// How its edge finds it.
    pub mount: HostMount,
    /// The modules its edge's crate declares.
    pub modules: &'static [RuntimeModule],
}

/// What a host's `Discoverable::register` attaches when its edge reads nothing
/// off it, so the run can tell the registration was made.
pub struct RegisteredHost;

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

/// What a host held as an instance does instead, on an edge that mounts none.
const INSTANCE_HINT: &str = "list it in a `#[module]`'s `providers`: its edge mounts what that \
                             registration attaches and builds its own instance from it";

/// File one `warn` per host of the app `container` was booted for that an edge
/// leaves unserved. Each entry is judged alone, since a host on two edges may
/// have one of them served: an entry with none of its modules imported is named
/// for those modules, seeded or registered by a hand-written module, so no
/// `#[module]` listing brought them. An instance on an edge mounting none is
/// said apart, whatever is imported: the import is not its fix.
pub(crate) fn report_unserved(container: &Container) {
    let Some(composition) = container.get::<Composition>() else {
        return;
    };
    let attached = container.attached_hosts();
    let mut instances = BTreeSet::new();
    let mut unserved: BTreeMap<_, BTreeSet<String>> = BTreeMap::new();
    for entry in inventory::iter::<HostRuntime>() {
        let host = (entry.host)();
        let held = container.holds(host);
        let registered = attached.contains(&host);
        if entry.mount == HostMount::Attached && !registered {
            if held {
                instances.insert(((entry.name)(), host));
            }
            continue;
        }
        let imported = entry
            .modules
            .iter()
            .any(|module| composition.imports((module.module)()));
        if (held || registered) && !imported {
            unserved
                .entry(((entry.name)(), host))
                .or_default()
                .extend(entry.modules.iter().map(|module| shorten((module.name)())));
        }
    }
    for (name, _) in instances {
        tracing::warn!(
            target: crate::target::APP,
            host = shorten(name),
            hint = INSTANCE_HINT,
            "host not served: its edge does not mount an instance",
        );
    }
    for ((name, _), import) in unserved {
        let import: Vec<String> = import.into_iter().collect();
        tracing::warn!(
            target: crate::target::APP,
            host = shorten(name),
            import = import.join(", "),
            "host not served: its edge's transport is not attached",
        );
    }
}
