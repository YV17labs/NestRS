//! Build-time validation of the module import graph (the access contract).
//!
//! The container is flat: any registered provider can be resolved by `TypeId`.
//! This pass enforces that `#[module(imports = [...])]` is honored — a
//! provider's `#[inject]` dependency (and its attribute-bound layers from
//! `#[use_guards]` / `#[use_filters]` / `#[use_interceptors]`) must be
//! provided by the provider's own module, by a module in its transitive
//! import closure, or by global infrastructure (seeds + factory outputs). A
//! cross-module reach that no import covers fails the boot with an
//! [`AccessGraphError`].
//!
//! Resolvers join the contract through module membership: a `#[resolver]`
//! listed in some reachable module's `providers = [...]` is governed like any
//! other provider; one in no reachable module is silently skipped from the
//! schema (a `tracing::warn` at boot surfaces it so leftover code does not
//! vanish without trace).
//!
//! Runtime [`Container::get`](crate::Container::get) /
//! [`get_dyn`](crate::Container::get_dyn) is an unchecked escape hatch by
//! design — the contract binds the declarative `#[inject]` surface, not
//! imperative resolution.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::container::{KeyedDependency, ProviderKey};

/// One provider declared in a module's `providers = [...]`, recorded by the
/// `#[module]` macro for the access-graph check.
pub struct ProviderDescriptor {
    pub name: &'static str,
    /// The container key this provider registers under:
    /// `TypeId::of::<Concrete>()` for an `#[injectable]`, or
    /// `TypeId::of::<Arc<dyn Trait>>()` for a `Foo as dyn Trait` binding.
    pub provides: fn() -> TypeId,
    /// `TypeId` of each bare `#[inject]` field plus each attribute-referenced
    /// layer (`#[use_guards]` / `#[use_filters]` / `#[use_interceptors]`).
    pub injects: fn() -> Vec<TypeId>,
    /// Each **keyed** `#[inject(key = "…")]` field, validated against the
    /// global keyed set. Empty for providers with no keyed dependency.
    pub injects_keyed: fn() -> Vec<KeyedDependency>,
}

/// Per-module descriptor submitted to the link-time registry by `#[module]`.
pub struct ModuleDescriptor {
    pub module: fn() -> TypeId,
    pub name: &'static str,
    /// Statically-typed imports only. Dynamic (`for_root(...)`) imports
    /// contribute only global infrastructure, never an injectable a provider
    /// could depend on.
    pub imports: &'static [fn() -> TypeId],
    pub providers: &'static [ProviderDescriptor],
}

inventory::collect!(ModuleDescriptor);

/// One `#[resolver]` linked into the binary, submitted to the link-time
/// registry by the macro. A resolver self-composes into the GraphQL schema
/// regardless of any module, so module membership is what brings its injected
/// dependencies under the access contract.
pub struct ResolverDescriptor {
    pub resolver: fn() -> TypeId,
    pub name: &'static str,
}

inventory::collect!(ResolverDescriptor);

/// A provider depends on something its module does not import and that is not
/// global infrastructure. Raised at boot by the access-graph validation.
#[derive(Debug, Error)]
#[error(
    "module access violation: `{consumer}` (in module `{module}`) depends on `{dependency}`, \
     but `{module}` imports no module that provides it. `{dependency}` is provided by `{owner}` \
     — add `{owner}` to `#[module(imports = [...])]` of `{module}`, or route the dependency \
     through a module `{module}` already imports."
)]
pub struct AccessGraphError {
    pub module: &'static str,
    pub consumer: &'static str,
    pub dependency: &'static str,
    pub owner: &'static str,
}

/// A provider's `#[inject(key = "…")]` keyed dependency has no keyed provider
/// registered as global infrastructure (a seed or a factory output). Raised at
/// boot by the keyed pass of the access-graph validation. Unlike a bare
/// dependency — deferred to the register-phase fixpoint when genuinely missing —
/// a keyed dependency is validated here so the failure is a clean boot error
/// naming **both** the type and the key, not a `get_keyed(...).expect(...)`
/// panic during construction.
#[derive(Debug, Error)]
#[error(
    "keyed dependency unreachable: `{consumer}` (in module `{module}`) injects `{type_name}` \
     keyed `{key}`, but no keyed provider for that (type, key) is registered. Register it as \
     global infrastructure — `App::builder().provide_keyed::<{type_name}>(\"{key}\", …)` or a \
     `ContainerBuilder::provide_keyed`/factory in a module reachable from the root."
)]
pub struct KeyedDependencyError {
    pub module: &'static str,
    pub consumer: &'static str,
    pub type_name: &'static str,
    pub key: &'static str,
}

/// Marker the schema-composing layer registers when an app actually composes
/// a resolver schema. The unreachable-resolver warn only fires when this is
/// present, so an app that links resolvers transitively but composes no
/// schema boots silent.
pub struct ResolverSchemaActive;

/// Provider keys reachable from the app's module tree, seeded into the
/// container so transports can module-gate their inventory: a `#[resolver]`
/// linked into the binary but living in no reachable module is silently
/// skipped from the GraphQL schema instead of failing the boot.
///
/// Includes every provider declared in a reachable module's
/// `providers = [...]` plus the global infrastructure keys.
pub struct ReachableProviders(pub std::collections::HashSet<TypeId>);

/// Validate the access graph: every provider's dependency must be reachable
/// from its module's import closure or be global infrastructure.
///
/// Pure over its inputs (no link-time registry access). `roots` without a
/// descriptor terminate a branch, making a hand-written root a no-op.
pub fn validate_access_graph(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> Result<(), AccessGraphError> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();

    // First binding wins; a key registered in two modules is a separate
    // (override) concern the container already warns about.
    let mut provided_by: HashMap<TypeId, (&'static str, &'static str)> = HashMap::new();
    for d in descriptors {
        for p in d.providers {
            provided_by
                .entry((p.provides)())
                .or_insert((p.name, d.name));
        }
    }

    for module_id in reachable(roots, &by_id) {
        let Some(desc) = by_id.get(&module_id) else {
            continue;
        };

        // Per-module BFS over the import closure (itself included). `global`
        // is checked separately, not cloned in. The module graph is shallow,
        // so single-pass closure memoization would not earn its complexity.
        let mut closure_keys = HashSet::new();
        for import_id in reachable(&[module_id], &by_id) {
            if let Some(imported) = by_id.get(&import_id) {
                for p in imported.providers {
                    closure_keys.insert((p.provides)());
                }
            }
        }

        for p in desc.providers {
            for dep in (p.injects)() {
                if global.contains(&dep) || closure_keys.contains(&dep) {
                    continue;
                }
                // A genuinely missing provider is rejected earlier by the
                // register-phase fixpoint; only flag a cross-module reach.
                if let Some((dependency, owner)) = provided_by.get(&dep) {
                    return Err(AccessGraphError {
                        module: desc.name,
                        consumer: p.name,
                        dependency,
                        owner,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Validate the **keyed** access graph: every reachable provider's
/// `#[inject(key = "…")]` dependency must be supplied by the global keyed set
/// (seeds + factory outputs). Keyed providers are configured imperatively, so
/// there is no per-module keyed declaration to reach through — a keyed
/// dependency is legal only when globally provided, and an unmet one is a clean
/// boot error naming type and key rather than a construction-time panic.
///
/// Pure over its inputs. Runs after [`validate_access_graph`] at boot.
pub fn validate_keyed_access_graph(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    global_keyed: &HashSet<ProviderKey>,
) -> Result<(), KeyedDependencyError> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();

    for module_id in reachable(roots, &by_id) {
        let Some(desc) = by_id.get(&module_id) else {
            continue;
        };
        for p in desc.providers {
            for dep in (p.injects_keyed)() {
                if global_keyed.contains(&dep.key) {
                    continue;
                }
                return Err(KeyedDependencyError {
                    module: desc.name,
                    consumer: p.name,
                    type_name: dep.type_name,
                    // A keyed dependency always carries a name; fall back
                    // defensively rather than unwrap on a framework path.
                    key: dep.key.name.unwrap_or("<unnamed>"),
                });
            }
        }
    }
    Ok(())
}

/// BFS over `imports` from `roots`, returning every module `TypeId` reached
/// (roots included). A `TypeId` without a descriptor terminates its branch.
fn reachable(roots: &[TypeId], by_id: &HashMap<TypeId, &ModuleDescriptor>) -> HashSet<TypeId> {
    let mut seen = HashSet::new();
    let mut stack = roots.to_vec();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(desc) = by_id.get(&id) {
            for import in desc.imports {
                stack.push((import)());
            }
        }
    }
    seen
}

/// Provider keys reachable from `roots` via the module import graph plus
/// `global`. Used at boot to seed [`ReachableProviders`] so transports can
/// module-gate their discovery. Pure over its inputs.
pub fn reachable_provider_ids(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> HashSet<TypeId> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();
    let mut keys = global.clone();
    for module_id in reachable(roots, &by_id) {
        if let Some(desc) = by_id.get(&module_id) {
            for p in desc.providers {
                keys.insert((p.provides)());
            }
        }
    }
    keys
}

/// Boot-time equivalent of [`reachable_provider_ids`] against the link-time
/// module registry.
pub(crate) fn reachable_provider_ids_from_inventory(
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> HashSet<TypeId> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    reachable_provider_ids(&descriptors, roots, global)
}

/// Linked resolvers that live in no module reachable from `roots`. Returned
/// for a boot-time `tracing::warn` — they are silently filtered from the
/// schema, so the warn keeps leftover code visible. Pure over its inputs.
pub fn unreachable_resolvers(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    resolvers: &[&ResolverDescriptor],
) -> Vec<&'static str> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();

    let mut reachable_keys = HashSet::new();
    for module_id in reachable(roots, &by_id) {
        if let Some(desc) = by_id.get(&module_id) {
            for p in desc.providers {
                reachable_keys.insert((p.provides)());
            }
        }
    }

    resolvers
        .iter()
        .filter(|r| !reachable_keys.contains(&(r.resolver)()))
        .map(|r| r.name)
        .collect()
}

/// Boot-time entry point: validate the link-time module registry against the
/// app's roots and global set. Returns the concrete [`AccessGraphError`] so a
/// caller can downcast the boot failure to the precise cause.
pub(crate) fn validate_from_inventory(
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> Result<(), AccessGraphError> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    validate_access_graph(&descriptors, roots, global)
}

/// Boot-time keyed pass over the link-time module registry — the
/// [`validate_keyed_access_graph`] counterpart of [`validate_from_inventory`].
pub(crate) fn validate_keyed_from_inventory(
    roots: &[TypeId],
    global_keyed: &HashSet<ProviderKey>,
) -> Result<(), KeyedDependencyError> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    validate_keyed_access_graph(&descriptors, roots, global_keyed)
}

/// Boot-time equivalent of [`unreachable_resolvers`] against the link-time
/// registry; backs the default `warn` and the opt-in strict-mode boot error.
pub(crate) fn unreachable_resolvers_from_inventory(roots: &[TypeId]) -> Vec<&'static str> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    let resolvers: Vec<&ResolverDescriptor> = inventory::iter::<ResolverDescriptor>().collect();
    unreachable_resolvers(&descriptors, roots, &resolvers)
}

/// Emit a `warn` for every linked-but-unreachable resolver — they are silently
/// filtered from the schema by module-gating.
pub(crate) fn warn_unreachable_resolvers_from_inventory(roots: &[TypeId]) {
    for name in unreachable_resolvers_from_inventory(roots) {
        tracing::warn!(
            target: "nest_rs::access_graph",
            resolver = name,
            hint = "add it to a feature module's `#[module(providers = [...])]` if you meant to expose it",
            "unreachable resolver skipped from the GraphQL schema",
        );
    }
}

/// Opt-in strict-mode boot failure raised by
/// [`AppBuilder::strict_resolver_membership`](crate::AppBuilder::strict_resolver_membership);
/// the default boot emits a `warn` instead.
#[derive(Debug, Error)]
#[error(
    "strict resolver-membership check failed: {0:?} linked into the binary but in no \
     reachable module. Add each to a reachable feature module's \
     `#[module(providers = [...])]`, or drop `strict_resolver_membership` if the link is \
     intentional (e.g. a workspace ships multiple apps with different surfaces)."
)]
pub struct UnreachableResolversError(pub Vec<&'static str>);

#[cfg(test)]
mod tests {
    use super::*;

    // Marker types for stable `TypeId`s; descriptors are hand-built so the
    // global `inventory` registry is untouched.
    struct AppMod;
    struct UsersMod;
    struct BillingMod;

    struct UsersService;
    struct BillingService;
    struct AppGuard;
    struct Db;
    struct OrgsResolver;

    fn no_deps() -> Vec<TypeId> {
        Vec::new()
    }

    fn no_keyed_deps() -> Vec<KeyedDependency> {
        Vec::new()
    }

    fn users_deps() -> Vec<TypeId> {
        vec![TypeId::of::<Db>()]
    }

    fn billing_deps() -> Vec<TypeId> {
        vec![TypeId::of::<UsersService>()]
    }

    fn users_module() -> ModuleDescriptor {
        ModuleDescriptor {
            module: || TypeId::of::<UsersMod>(),
            name: "UsersModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "UsersService",
                provides: || TypeId::of::<UsersService>(),
                injects: users_deps,
                injects_keyed: no_keyed_deps,
            }],
        }
    }

    fn global() -> HashSet<TypeId> {
        HashSet::from([TypeId::of::<Db>()])
    }

    #[test]
    fn dependency_on_global_infrastructure_passes() {
        let users = users_module();
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[|| TypeId::of::<UsersMod>()],
            providers: &[],
        };
        let descriptors = [&app, &users];
        validate_access_graph(&descriptors, &[TypeId::of::<AppMod>()], &global())
            .expect("a dependency on global infrastructure is always reachable");
    }

    #[test]
    fn same_module_dependency_passes() {
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[
                ProviderDescriptor {
                    name: "AppAbility",
                    provides: || TypeId::of::<UsersService>(),
                    injects: no_deps,
                    injects_keyed: no_keyed_deps,
                },
                ProviderDescriptor {
                    name: "AppGuard",
                    provides: || TypeId::of::<AppGuard>(),
                    injects: billing_deps,
                    injects_keyed: no_keyed_deps,
                },
            ],
        };
        validate_access_graph(&[&app], &[TypeId::of::<AppMod>()], &HashSet::new())
            .expect("a provider may depend on another provider of the same module");
    }

    #[test]
    fn imported_module_dependency_passes() {
        let users = users_module();
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[|| TypeId::of::<UsersMod>()],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                injects_keyed: no_keyed_deps,
            }],
        };
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[|| TypeId::of::<BillingMod>(), || TypeId::of::<UsersMod>()],
            providers: &[],
        };
        validate_access_graph(
            &[&app, &billing, &users],
            &[TypeId::of::<AppMod>()],
            &global(),
        )
        .expect("an imported module's provider is reachable");
    }

    #[test]
    fn unimported_cross_module_dependency_is_rejected() {
        let users = users_module();
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                injects_keyed: no_keyed_deps,
            }],
        };
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[|| TypeId::of::<BillingMod>(), || TypeId::of::<UsersMod>()],
            providers: &[],
        };
        let err = validate_access_graph(
            &[&app, &billing, &users],
            &[TypeId::of::<AppMod>()],
            &global(),
        )
        .expect_err("reaching an unimported module must fail");

        assert_eq!(err.consumer, "BillingService");
        assert_eq!(err.module, "BillingModule");
        assert_eq!(err.dependency, "UsersService");
        assert_eq!(err.owner, "UsersModule");
        let msg = err.to_string();
        assert!(msg.contains("BillingService"), "{msg}");
        assert!(msg.contains("UsersModule"), "{msg}");
    }

    #[test]
    fn unimported_module_outside_the_root_tree_is_not_validated() {
        // BillingModule has a violation but is not reachable from the root.
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                injects_keyed: no_keyed_deps,
            }],
        };
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[],
        };
        validate_access_graph(
            &[&app, &billing],
            &[TypeId::of::<AppMod>()],
            &HashSet::new(),
        )
        .expect("a module outside the root's import tree is not validated");
    }

    #[test]
    fn hand_written_root_without_descriptor_is_a_noop() {
        validate_access_graph(&[], &[TypeId::of::<AppMod>()], &HashSet::new())
            .expect("a root with no descriptor validates trivially");
    }

    fn orgs_resolver_desc() -> ResolverDescriptor {
        ResolverDescriptor {
            resolver: || TypeId::of::<OrgsResolver>(),
            name: "OrgsResolver",
        }
    }

    #[test]
    fn listed_resolver_is_reachable() {
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "OrgsResolver",
                provides: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
                injects_keyed: no_keyed_deps,
            }],
        };
        let keys = reachable_provider_ids(&[&app], &[TypeId::of::<AppMod>()], &HashSet::new());
        assert!(keys.contains(&TypeId::of::<OrgsResolver>()));
        let resolver = orgs_resolver_desc();
        let leftover = unreachable_resolvers(&[&app], &[TypeId::of::<AppMod>()], &[&resolver]);
        assert!(leftover.is_empty());
    }

    #[test]
    fn unlisted_resolver_is_reported_unreachable() {
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[],
        };
        let resolver = orgs_resolver_desc();
        let leftover = unreachable_resolvers(&[&app], &[TypeId::of::<AppMod>()], &[&resolver]);
        assert_eq!(leftover, vec!["OrgsResolver"]);
    }

    #[test]
    fn resolver_listed_only_in_unreachable_module_is_unreachable() {
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "OrgsResolver",
                provides: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
                injects_keyed: no_keyed_deps,
            }],
        };
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[],
        };
        let keys = reachable_provider_ids(
            &[&app, &billing],
            &[TypeId::of::<AppMod>()],
            &HashSet::new(),
        );
        assert!(!keys.contains(&TypeId::of::<OrgsResolver>()));
        let resolver = orgs_resolver_desc();
        let leftover =
            unreachable_resolvers(&[&app, &billing], &[TypeId::of::<AppMod>()], &[&resolver]);
        assert_eq!(leftover, vec!["OrgsResolver"]);
    }

    #[test]
    fn global_keys_are_reachable() {
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[],
        };
        let keys = reachable_provider_ids(&[&app], &[TypeId::of::<AppMod>()], &global());
        assert!(keys.contains(&TypeId::of::<Db>()));
    }

    // A stand-in for the keyed `OAuth2Client` case: one concrete type injected
    // twice, disambiguated by key.
    struct OAuth2Client;

    fn github_dep() -> Vec<KeyedDependency> {
        vec![KeyedDependency {
            key: ProviderKey::named::<OAuth2Client>("github"),
            type_name: "OAuth2Client",
        }]
    }

    fn keyed_consumer_module() -> ModuleDescriptor {
        ModuleDescriptor {
            module: || TypeId::of::<UsersMod>(),
            name: "UsersModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "SocialLoginService",
                provides: || TypeId::of::<UsersService>(),
                injects: no_deps,
                injects_keyed: github_dep,
            }],
        }
    }

    #[test]
    fn keyed_dependency_supplied_globally_passes() {
        let users = keyed_consumer_module();
        let global_keyed =
            HashSet::from([ProviderKey::named::<OAuth2Client>("github")]);
        validate_keyed_access_graph(&[&users], &[TypeId::of::<UsersMod>()], &global_keyed)
            .expect("a globally-seeded keyed provider satisfies the keyed dependency");
    }

    #[test]
    fn unmet_keyed_dependency_is_rejected_naming_type_and_key() {
        let users = keyed_consumer_module();
        let err = validate_keyed_access_graph(
            &[&users],
            &[TypeId::of::<UsersMod>()],
            &HashSet::new(),
        )
        .expect_err("a keyed dependency with no keyed provider must fail");
        assert_eq!(err.consumer, "SocialLoginService");
        assert_eq!(err.module, "UsersModule");
        assert_eq!(err.type_name, "OAuth2Client");
        assert_eq!(err.key, "github");
        let msg = err.to_string();
        assert!(msg.contains("OAuth2Client"), "names the type: {msg}");
        assert!(msg.contains("github"), "names the key: {msg}");
    }

    #[test]
    fn wrong_key_does_not_satisfy_a_keyed_dependency() {
        // Only the exact `(type, key)` counts — a different key of the same
        // type leaves the dependency unmet.
        let users = keyed_consumer_module();
        let global_keyed =
            HashSet::from([ProviderKey::named::<OAuth2Client>("google")]);
        let err = validate_keyed_access_graph(
            &[&users],
            &[TypeId::of::<UsersMod>()],
            &global_keyed,
        )
        .expect_err("the `google` key must not satisfy a `github` dependency");
        assert_eq!(err.key, "github");
    }
}
