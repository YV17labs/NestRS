//! [`EdgeRuntime`] — what an edge's hosts run on, so [`App::run`](crate::App::run)
//! names each host the app reaches whose runtime it does not attach.

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::sync::Arc;

use crate::access::__private::ModuleDescriptor;
use crate::access::{Composition, ReachableProviders};
use crate::container::Container;
use crate::discovery::Discovery;
use crate::edge::Edge;
use crate::transport::{Transport, TransportContribution};

/// Each host of an edge, by its type and the name a line gives it.
pub type EdgeHosts = Vec<(TypeId, Cow<'static, str>)>;

/// An edge whose hosts run only on a runtime the app attaches — its transport,
/// or its mount on one — submitted once by the edge's crate.
pub struct EdgeRuntime {
    /// The edge its hosts serve.
    pub edge: Edge,
    /// The module attaching the runtime, as an adapter's `imports` writes it.
    pub import: &'static str,
    /// Whether the app `container` was booted for attaches the runtime.
    pub attached: fn(&Container) -> bool,
    /// Each host of the edge this binary links or the app registered, by type
    /// and name: the kernel keeps those the app reaches.
    pub hosts: fn(&Container) -> EdgeHosts,
}

inventory::collect!(EdgeRuntime);

/// The health probes, under which alone a `#[liveness]`, `#[readiness]` or
/// `#[startup]` indicator runs — submitted once by the health crate, so the
/// app names each indicator's host it reaches without them.
pub struct ProbeRuntime {
    /// The module mounting the probes, as an app's `imports` writes it.
    pub import: &'static str,
    /// Whether the app `container` was booted for mounts the probes.
    pub attached: fn(&Container) -> bool,
    /// Each indicator's host this binary links, by type and name: the kernel
    /// keeps those the app reaches.
    pub hosts: fn(&Container) -> EdgeHosts,
}

inventory::collect!(ProbeRuntime);

/// Whether the app `container` was booted for attaches a transport of type `T`.
pub fn attaches_transport<T: Transport>(container: &Container) -> bool {
    Discovery::new(container)
        .meta::<TransportContribution>()
        .iter()
        .any(|contribution| contribution.meta.is::<T>())
}

/// Each host the app's modules attached an `M` to, named by `name`; an `M`
/// it answers `None` for is no host of the edge.
pub fn registered_hosts<M: Any + Send + Sync>(
    container: &Container,
    name: impl Fn(&M) -> Option<Cow<'static, str>>,
) -> EdgeHosts {
    Discovery::new(container)
        .meta::<M>()
        .into_iter()
        .filter_map(|found| Some((found.provider_type_id?, name(&found.meta)?)))
        .collect()
}

/// What an ungated container is refused naming.
const HOSTS: &str = "edge host";

/// File one `warn` per host the app `container` was booted for reaches whose
/// edge's runtime it does not attach, then per indicator's host it reaches
/// without the health probes, naming the host, the module listing it and the
/// import serving it. A container no boot sealed is refused.
pub(crate) fn report_unserved(container: &Container) -> anyhow::Result<()> {
    let unserved = Unserved {
        container,
        reachable: ReachableProviders::of(container, HOSTS)?,
        composition: container.get::<Composition>(),
        descriptors: inventory::iter::<ModuleDescriptor>().collect(),
    };
    let mut runtimes: Vec<&EdgeRuntime> = inventory::iter::<EdgeRuntime>().collect();
    runtimes.sort_by_key(|runtime| Edge::ALL.iter().position(|edge| *edge == runtime.edge));
    for runtime in runtimes {
        if (runtime.attached)(container) {
            continue;
        }
        for (name, module) in unserved.hosts(runtime.hosts) {
            tracing::warn!(
                target: crate::target::APP,
                edge = runtime.edge.as_str(),
                host = &*name,
                module,
                import = runtime.import,
                "host not served: its edge's transport is not attached",
            );
        }
    }
    for probes in inventory::iter::<ProbeRuntime>() {
        if (probes.attached)(container) {
            continue;
        }
        for (name, module) in unserved.hosts(probes.hosts) {
            tracing::warn!(
                target: crate::target::APP,
                host = &*name,
                module,
                import = probes.import,
                "host not served: the health probes are not mounted",
            );
        }
    }
    Ok(())
}

/// What tells which of a runtime's hosts the app reaches, and who lists each.
struct Unserved<'c> {
    container: &'c Container,
    reachable: Arc<ReachableProviders>,
    composition: Option<Arc<Composition>>,
    descriptors: Vec<&'static ModuleDescriptor>,
}

impl Unserved<'_> {
    /// Each host `hosts` names that the app reaches, once, by name, with the
    /// module listing it.
    fn hosts(
        &self,
        hosts: fn(&Container) -> EdgeHosts,
    ) -> Vec<(Cow<'static, str>, Option<&'static str>)> {
        let mut hosts = hosts(self.container);
        hosts.retain(|(host, _)| self.reachable.reaches(*host));
        // By name, then type, so a host's entries sit together for the dedup
        // even where two hosts share a name.
        hosts.sort_unstable_by(|(a, a_name), (b, b_name)| a_name.cmp(b_name).then(a.cmp(b)));
        hosts.dedup_by_key(|(host, _)| *host);
        hosts
            .into_iter()
            .map(|(host, name)| {
                let module = listing_module(&self.descriptors, self.composition.as_deref(), host);
                (name, module)
            })
            .collect()
    }
}

/// The module the app reaches that lists `host` in its `providers`; any
/// module listing it when the container carries no composition.
fn listing_module(
    descriptors: &[&ModuleDescriptor],
    composition: Option<&Composition>,
    host: TypeId,
) -> Option<&'static str> {
    descriptors
        .iter()
        .filter(|module| module.providers.iter().any(|p| (p.provider)() == host))
        .find(|module| composition.is_none_or(|c| c.reaches((module.module)())))
        .map(|module| module.name)
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::collections::HashSet;

    use super::listing_module;
    use crate::access::__private::{ModuleDescriptor, ProviderDescriptor};
    use crate::access::Composition;

    struct ServedModule;
    struct OtherModule;
    struct Tasks;

    fn nothing() -> Vec<TypeId> {
        Vec::new()
    }
    fn no_names() -> Vec<&'static str> {
        Vec::new()
    }
    fn no_pairs() -> Vec<(TypeId, &'static str)> {
        Vec::new()
    }

    const TASKS: ProviderDescriptor = ProviderDescriptor {
        name: "Tasks",
        provides: || TypeId::of::<Tasks>(),
        provider: || TypeId::of::<Tasks>(),
        also_provides: no_pairs,
        injects: nothing,
        inject_names: no_names,
        injects_optional: nothing,
        inject_optional_names: no_names,
        injects_per_request: no_pairs,
    };

    /// Two modules list `Tasks`; the app's root is `ServedModule`.
    fn listed_twice() -> (Vec<ModuleDescriptor>, Composition) {
        let modules = vec![
            ModuleDescriptor {
                module: || TypeId::of::<OtherModule>(),
                name: "OtherModule",
                imports: &[],
                providers: &[TASKS],
            },
            ModuleDescriptor {
                module: || TypeId::of::<ServedModule>(),
                name: "ServedModule",
                imports: &[],
                providers: &[TASKS],
            },
        ];
        let refs: Vec<&ModuleDescriptor> = modules.iter().collect();
        let composition = Composition::from_descriptors(
            &refs,
            &[(TypeId::of::<ServedModule>(), "app::ServedModule")],
            HashSet::new(),
        );
        (modules, composition)
    }

    #[test]
    fn the_module_named_is_the_one_the_app_reaches() {
        let (modules, composition) = listed_twice();
        let refs: Vec<&ModuleDescriptor> = modules.iter().collect();

        assert_eq!(
            listing_module(&refs, Some(&composition), TypeId::of::<Tasks>()),
            Some("ServedModule"),
        );
    }

    #[test]
    fn a_host_no_module_lists_names_none() {
        let (modules, composition) = listed_twice();
        let refs: Vec<&ModuleDescriptor> = modules.iter().collect();

        assert_eq!(
            listing_module(&refs, Some(&composition), TypeId::of::<ServedModule>()),
            None,
        );
    }
}
