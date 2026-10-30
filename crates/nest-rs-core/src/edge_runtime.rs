//! [`EdgeRuntime`] — what an edge's hosts run on, so [`App::run`](crate::App::run)
//! names each host the app reaches whose runtime it does not attach.

use std::any::TypeId;
use std::borrow::Cow;

use crate::access::__private::ModuleDescriptor;
use crate::access::{Composition, ReachableProviders};
use crate::container::Container;
use crate::discovery::Discovery;
use crate::operation_log::Edge;
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

/// Whether the app `container` was booted for attaches a transport of type `T`.
pub fn attaches_transport<T: Transport>(container: &Container) -> bool {
    Discovery::new(container)
        .meta::<TransportContribution>()
        .iter()
        .any(|contribution| contribution.meta.is::<T>())
}

/// What an ungated container is refused naming.
const HOSTS: &str = "edge host";

/// File one `warn` per host the app `container` was booted for reaches whose
/// edge's runtime it does not attach, naming the host, the module listing it
/// and the import serving it. A container no boot sealed is refused.
pub(crate) fn report_unserved(container: &Container) -> anyhow::Result<()> {
    let reachable = ReachableProviders::of(container, HOSTS)?;
    let composition = container.get::<Composition>();
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    let mut runtimes: Vec<&EdgeRuntime> = inventory::iter::<EdgeRuntime>().collect();
    runtimes.sort_by_key(|runtime| Edge::ALL.iter().position(|edge| *edge == runtime.edge));
    for runtime in runtimes {
        if (runtime.attached)(container) {
            continue;
        }
        let mut hosts = (runtime.hosts)(container);
        hosts.retain(|(host, _)| reachable.reaches(*host));
        hosts.sort_unstable_by(|(_, a), (_, b)| a.cmp(b));
        hosts.dedup_by_key(|(host, _)| *host);
        for (host, name) in hosts {
            let module = listing_module(&descriptors, composition.as_deref(), host);
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
    Ok(())
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
