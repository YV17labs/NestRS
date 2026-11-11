//! [`ProbeRuntime`] — what an indicator runs under, so [`App::run`](crate::App::run)
//! names each indicator's host the app reaches without the health probes.

use std::any::TypeId;

use crate::access::__private::ModuleDescriptor;
use crate::access::{Composition, ReachableProviders};
use crate::container::Container;

/// The health probes, under which alone a `#[liveness]`, `#[readiness]` or
/// `#[startup]` indicator runs — submitted once by the health crate, so the
/// app names each indicator's host it reaches without them.
pub struct ProbeRuntime {
    /// The module mounting the probes, as an app's `imports` writes it.
    pub import: &'static str,
    /// Whether the app `container` was booted for mounts the probes.
    pub attached: fn(&Container) -> bool,
    /// Each indicator's host this binary links, by the name a line gives it and
    /// its type: the kernel keeps those the app reaches.
    pub hosts: fn() -> Vec<(&'static str, TypeId)>,
}

inventory::collect!(ProbeRuntime);

/// What an ungated container is refused naming.
const HOSTS: &str = "indicator host";

/// File one `warn` per indicator's host the app `container` was booted for
/// reaches without the health probes, naming the host, the module listing it
/// and the import serving it. A container no boot sealed is refused.
pub(crate) fn report_unserved(container: &Container) -> anyhow::Result<()> {
    let reachable = ReachableProviders::of(container, HOSTS)?;
    let composition = container.get::<Composition>();
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    for probes in inventory::iter::<ProbeRuntime>() {
        if (probes.attached)(container) {
            continue;
        }
        let mut hosts = (probes.hosts)();
        hosts.retain(|&(_, host)| reachable.reaches(host));
        hosts.sort_unstable();
        hosts.dedup();
        for (name, host) in hosts {
            tracing::warn!(
                target: crate::target::APP,
                host = name,
                module = listing_module(&descriptors, composition.as_deref(), host),
                import = probes.import,
                "host not served: the health probes are not mounted",
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
    fn no_remedies() -> Vec<crate::InjectedRemedy> {
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
        inject_remedies: no_remedies,
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
