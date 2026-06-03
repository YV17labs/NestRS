//! Build-time validation of the module import graph (the access contract).
//!
//! The container is a flat `HashMap<TypeId, Arc<…>>`: any provider can resolve
//! any other provider that is registered, regardless of which module declared
//! it. Rust visibility (`pub(crate)` impls behind exported traits) covers
//! encapsulation axis 1 — *what a module can name*. This pass covers axis 2 —
//! *which modules' providers a module is allowed to reach*: it turns
//! `#[module(imports = [...])]` into an **enforced access contract**.
//!
//! The `#[module]` macro submits one [`ModuleDescriptor`] per module to the
//! link-time [`inventory`] registry (the same mechanism `#[hooks]` and GraphQL
//! composition use), recording the module's bare-type imports and its
//! providers' container keys + declared dependencies. At boot,
//! [`App`](crate::App) walks the import graph from the root module(s) and
//! checks that every provider's dependency is reachable — provided by the
//! provider's own module, by a module in its transitive import closure, or by
//! the **global** set (seeds + factory outputs: everything present before the
//! register phase, i.e. the app's shared infrastructure). A dependency that
//! crosses a non-imported module boundary fails the boot with an
//! [`AccessGraphError`] naming the offending provider, the dependency, and the
//! module to import.
//!
//! Only `#[module]`-decorated modules participate; a hand-written `impl Module`
//! emits no descriptor and is exempt. Every provider listed in a module's
//! `providers = [...]` is under contract — `#[injectable]`, `#[interceptor]`,
//! guards, `#[cron_job]`, `#[processor]`, `#[controller]`, `#[mcp]`,
//! `#[resolver]` — via
//! [`Discoverable::injected`](crate::Discoverable::injected), which reports a
//! provider's `#[inject]` keys whether it is built eagerly or later from the
//! assembled container.
//!
//! `injected` also reports a provider's **attribute-referenced layers** — the
//! guards/filters/interceptors a `#[controller]`/`#[routes]` (or a `#[gateway]`/
//! `#[messages]`) binds with `#[use_guards]` / `#[use_filters]` /
//! `#[use_interceptors]`, at both the controller/gateway and per-route/per-message
//! scope. Each is resolved from the container at mount (`Container::get::<P>`)
//! exactly like an `#[inject]` dependency, so it is held to the same contract: a
//! layer registered in a module the consumer does not import fails the boot with
//! the named [`AccessGraphError`] instead of being resolved silently through the
//! flat container (a cross-module encapsulation breach). The macros fold the layer
//! `TypeId`s into the consumer's `injected`, so the graph check below covers them
//! with no special-casing.
//!
//! # Resolvers join the contract through module membership
//!
//! A `#[resolver]` self-composes into the GraphQL schema through the link-time
//! registry. Module-gating filters that registry at schema-build time —
//! `nestrs-graphql` reads [`ReachableProviders`] from the container and keeps
//! only resolvers whose `TypeId` is in the reachable set — so a resolver
//! listed in `providers = [...]` of a reachable module is in the schema, and
//! one in no reachable module is silently skipped (a `tracing::warn` surfaces
//! it at boot via [`warn_unreachable_resolvers_from_inventory`] so leftover
//! code does not disappear without trace). `#[resolver]` emits an `impl
//! Discoverable` (a no-op `register` — the schema builds the resolver from
//! the assembled container — and an `injected` reporting its `#[inject]`
//! keys, its `#[use_guards]` resolver/operation guards, and the
//! container-resolved `&Service` dependencies of its `#[field]`s), so a
//! listed resolver produces a [`ProviderDescriptor`] like any other and the
//! graph check above governs its dependencies. An unreachable resolver's
//! dependencies are never resolved (no schema entry to drive them), so the
//! contract has nothing to check there. (A `#[field]`'s `&DataLoader<…>` is
//! request-scoped — read from the GraphQL context, not the container — so it
//! is not an injected key and stays out of the graph, like the dataloaders
//! themselves; loaders are module-gated the same way as resolvers, by their
//! owner service's `TypeId`.)
//!
//! # What the contract does *not* cover (one deliberate boundary)
//!
//! The contract governs **declarative `#[inject]` dependencies of module
//! providers** (resolvers included, per above). One path falls outside it by
//! design — named so callers are not misled into thinking the check is total:
//!
//! 1. **Runtime [`Container::get`](crate::Container::get) /
//!    [`get_dyn`](crate::Container::get_dyn) is an unchecked escape hatch** — the
//!    `ModuleRef.get()` analog. The flat container resolves by `TypeId` with no
//!    caller identity, so a provider that reaches the `Container` directly (a
//!    `#[inject] container: Container`, a transport, a lazily-built handler) can
//!    fetch anything registered, bypassing the import graph. This is inherent to
//!    a flat container and is the intended override path; the contract binds the
//!    *declarative* surface (`#[inject]`), not imperative resolution.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};

use thiserror::Error;

/// One provider declared in a module's `providers = [...]`, recorded by the
/// `#[module]` macro for the access-graph check.
pub struct ProviderDescriptor {
    /// Human-readable label for diagnostics (`"UsersService"`,
    /// `"dyn WeatherProvider"`).
    pub name: &'static str,
    /// The container key this provider registers under — what it can satisfy
    /// for others: `TypeId::of::<Concrete>()` for an `#[injectable]`, or
    /// `TypeId::of::<Arc<dyn Trait>>()` for a `Foo as dyn Trait` binding.
    pub provides: fn() -> TypeId,
    /// The provider's declared injection dependencies
    /// ([`Discoverable::injected`](crate::Discoverable::injected)) — the
    /// `TypeId` of each `#[inject]` field *plus* each attribute-referenced layer
    /// (`#[use_guards]` / `#[use_filters]` / `#[use_interceptors]`), for *every*
    /// provider kind under contract (`#[injectable]`, `#[interceptor]`, guards,
    /// `#[cron_job]`, `#[processor]`, `#[controller]`, `#[mcp]`), regardless of
    /// whether it is built eagerly or later from the assembled container.
    pub injects: fn() -> Vec<TypeId>,
}

/// Per-module descriptor submitted to the link-time registry by `#[module]`.
pub struct ModuleDescriptor {
    /// The module struct's own `TypeId`.
    pub module: fn() -> TypeId,
    /// The module struct name, for diagnostics.
    pub name: &'static str,
    /// `TypeId`s of the **statically-typed** modules this one imports. Dynamic
    /// (`for_root(...)`) imports are omitted: they contribute only global
    /// infrastructure (factory outputs — a DB pool, a queue connection) or
    /// self-mounted metadata, never an injectable a provider could depend on.
    pub imports: &'static [fn() -> TypeId],
    /// The providers this module declares in `providers = [...]`.
    pub providers: &'static [ProviderDescriptor],
}

inventory::collect!(ModuleDescriptor);

/// One `#[resolver]` linked into the binary, submitted to the link-time registry
/// by the macro. A resolver self-composes into the GraphQL schema regardless of
/// any module (so it is always live), so — to bring its injected dependencies
/// under the contract — it must be a member of a module (listed in
/// `providers = [...]`), which gives it the import closure to check against. This
/// descriptor lets the boot verify that membership exists.
pub struct ResolverDescriptor {
    /// The resolver struct's `TypeId` — must match a provider key of some module
    /// reachable from the application root.
    pub resolver: fn() -> TypeId,
    /// The resolver struct name, for diagnostics.
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
    /// The module whose import list is incomplete.
    pub module: &'static str,
    /// The provider declaring the offending dependency.
    pub consumer: &'static str,
    /// The dependency that is out of reach.
    pub dependency: &'static str,
    /// The module that provides the dependency and should be imported.
    pub owner: &'static str,
}

/// Marker the schema-composing layer registers when an app actually composes
/// the resolver schema, so the boot knows the link-time [`ResolverDescriptor`]
/// registry is the home of these resolvers. The unreachable-resolver warn
/// only fires then: a `#[resolver]` is part of *a* schema only when one is
/// composed, so an app that links resolvers transitively (e.g. through a
/// shared library) but composes no schema is not their home and should boot
/// silent. The surface crate that builds the schema provides this
/// (`builder.provide(ResolverSchemaActive)`); the boot calls
/// [`warn_unreachable_resolvers_from_inventory`] only when it is present.
pub struct ResolverSchemaActive;

/// The set of provider keys an app's module tree reaches. Seeded into the
/// container at boot by [`App::new`](crate::App::new) /
/// [`AppBuilder::build`](crate::AppBuilder::build) so a transport's discovery
/// can **module-gate** its inventory: a `#[resolver]` linked into the binary
/// but living in no reachable module is silently skipped from the GraphQL
/// schema instead of failing the boot, letting one workspace ship apps that
/// expose different surfaces of the same feature.
///
/// The set includes every provider declared in a reachable module's
/// `providers = [...]` plus the global infrastructure keys (seeds + factory
/// outputs). Empty when no module roots the access graph (a hand-written
/// `App::new::<MyModule>` whose `MyModule` is not `#[module]`-decorated).
pub struct ReachableProviders(pub std::collections::HashSet<TypeId>);

/// Validate the access graph: every provider's dependency must be reachable
/// from its module's import closure or be global infrastructure. Pure over its
/// inputs (no link-time registry access), so it is exhaustively unit-testable.
///
/// - `descriptors` — every module descriptor in the binary.
/// - `roots` — the application's root module `TypeId`(s); validation covers
///   only modules reachable from these (a linked-but-unimported module is not
///   the running app's concern). Roots without a descriptor terminate a branch,
///   making a hand-written root a no-op.
/// - `global` — container keys present before the register phase (seeds +
///   factory outputs); reachable from any module.
pub fn validate_access_graph(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> Result<(), AccessGraphError> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();

    // Every provider key → (label, owning module name), for the "import X"
    // suggestion. First binding wins; a key registered in two modules is a
    // separate (override) concern the container already warns about.
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

        // Provider keys reachable from this module's transitive import closure
        // (itself included). `global` is checked separately below rather than
        // copied in, so it is not cloned per module. The per-module walk is a
        // plain cycle-tolerant BFS: boot-time work over a shallow module graph,
        // so a single-pass closure memoization would not earn its complexity.
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
                // Not reachable. If some other module provides it, that is the
                // violation. If no module provides it, the dependency is either
                // global (already handled) or genuinely missing — and a missing
                // provider is rejected earlier by the register-phase fixpoint,
                // so we skip rather than risk a false positive.
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

/// Compute the set of provider keys (container `TypeId`s) reachable from
/// `roots` via the module import graph plus `global`. Used at boot to seed
/// [`ReachableProviders`] so transports can filter their discovery: a
/// `#[resolver]`'s `TypeId` is its provider key (it lists itself in
/// `providers = [...]`), so a transport that holds an inventory entry per
/// resolver checks membership by looking up the resolver's `TypeId` here.
/// Pure over its inputs, like [`validate_access_graph`].
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

/// Compute reachable provider keys against the link-time module registry — the
/// boot-time equivalent of [`reachable_provider_ids`] called from
/// [`App::new`](crate::App::new) / [`AppBuilder::build`](crate::AppBuilder::build).
pub(crate) fn reachable_provider_ids_from_inventory(
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> HashSet<TypeId> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    reachable_provider_ids(&descriptors, roots, global)
}

/// Identify linked resolvers that live in no module reachable from `roots` —
/// they would have been a hard boot failure before module-gating, when every
/// linked resolver had to be listed in a reachable module so its `#[inject]`
/// dependencies were covered by the access contract. With module-gating, an
/// unreachable resolver is silently skipped from the schema (its `TypeId` is
/// not in [`ReachableProviders`]), so its dependencies are never resolved —
/// no contract to check. The names are returned for a `tracing::warn` at
/// boot, surfacing what was filtered without aborting.
///
/// Pure over its inputs, like [`validate_access_graph`].
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

/// Validate the link-time module registry against the app's roots and global
/// set. Called by [`App`](crate::App) at boot, alongside
/// [`warn_unreachable_resolvers_from_inventory`]. Kept returning the concrete
/// [`AccessGraphError`] (rather than a unified enum) so a caller can `downcast`
/// the boot failure to the precise cause.
pub(crate) fn validate_from_inventory(
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> Result<(), AccessGraphError> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    validate_access_graph(&descriptors, roots, global)
}

/// Collect the names of every linked resolver living in no reachable module,
/// against the link-time registry. The boot path uses this for both the
/// default `warn` ([`warn_unreachable_resolvers_from_inventory`]) and the
/// opt-in strict-mode boot error
/// ([`AppBuilder::strict_resolver_membership`](crate::AppBuilder::strict_resolver_membership)).
pub(crate) fn unreachable_resolvers_from_inventory(roots: &[TypeId]) -> Vec<&'static str> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    let resolvers: Vec<&ResolverDescriptor> = inventory::iter::<ResolverDescriptor>().collect();
    unreachable_resolvers(&descriptors, roots, &resolvers)
}

/// Log a `warn` for every linked resolver living in no reachable module — they
/// will be silently filtered from the schema by module-gating, and the warn
/// surfaces what was filtered (a leftover `#[resolver]` in a feature folder no
/// app imports is almost certainly a mistake). Called by [`App`](crate::App)
/// at boot, after [`validate_from_inventory`].
pub(crate) fn warn_unreachable_resolvers_from_inventory(roots: &[TypeId]) {
    for name in unreachable_resolvers_from_inventory(roots) {
        tracing::warn!(
            target: "nestrs::access",
            resolver = name,
            "resolver linked into the binary but in no reachable module — \
             skipped from the GraphQL schema; add it to a feature module's \
             `#[module(providers = [...])]` if you meant to expose it",
        );
    }
}

/// Opt-in strict-mode boot failure raised by
/// [`AppBuilder::strict_resolver_membership`](crate::AppBuilder::strict_resolver_membership)
/// when any linked resolver lives in no reachable module. The default boot
/// emits a `warn` instead — see
/// [`warn_unreachable_resolvers_from_inventory`].
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

    // Distinct marker types to mint stable `TypeId`s and module identities for
    // the graph under test — the descriptors are built by hand here, exactly as
    // the `#[module]` macro would emit them, without touching the global
    // `inventory` registry (which is shared across the whole test binary).
    struct AppMod;
    struct UsersMod;
    struct BillingMod;

    struct UsersService;
    struct BillingService;
    struct AppGuard;
    struct Db; // stands in for a seeded / factory-built infrastructure value.
    struct OrgsResolver; // stands in for a `#[resolver]` type.

    fn no_deps() -> Vec<TypeId> {
        Vec::new()
    }

    /// `UsersService` depends on the global `Db`.
    fn users_deps() -> Vec<TypeId> {
        vec![TypeId::of::<Db>()]
    }

    /// `BillingService` depends on `UsersService` (which lives in `UsersMod`).
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
            }],
        }
    }

    fn global() -> HashSet<TypeId> {
        HashSet::from([TypeId::of::<Db>()])
    }

    #[test]
    fn dependency_on_global_infrastructure_passes() {
        // UsersService -> Db, Db is global. No import needed.
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
                    provides: || TypeId::of::<UsersService>(), // reuse as a marker key
                    injects: no_deps,
                },
                ProviderDescriptor {
                    name: "AppGuard",
                    provides: || TypeId::of::<AppGuard>(),
                    injects: billing_deps, // depends on the key above
                },
            ],
        };
        validate_access_graph(&[&app], &[TypeId::of::<AppMod>()], &HashSet::new())
            .expect("a provider may depend on another provider of the same module");
    }

    #[test]
    fn imported_module_dependency_passes() {
        // BillingService -> UsersService, and BillingModule imports UsersModule.
        let users = users_module();
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[|| TypeId::of::<UsersMod>()],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                injects: billing_deps,
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
        // BillingService -> UsersService, but BillingModule does NOT import
        // UsersModule (they are only siblings under AppModule). Reaching across
        // that boundary in a flat container is exactly what the access contract forbids.
        let users = users_module();
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[], // <- the missing import
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                injects: billing_deps,
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
        // BillingModule has a violation but is not reachable from the root, so
        // it is not the running app's concern and must not fail the boot.
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                injects: billing_deps, // needs UsersService, unreachable
            }],
        };
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[], // does not import BillingModule
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
        // No descriptor matches the root TypeId → nothing to validate.
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
        // A resolver listed in `providers` is a member of its reachable module —
        // its `TypeId` appears in `reachable_provider_ids`, so a transport that
        // module-gates by membership keeps it in the schema.
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "OrgsResolver",
                provides: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
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
        // The resolver is linked (hence in the schema before module-gating) but
        // listed in no module — module-gating skips it, and `unreachable_resolvers`
        // surfaces it for the boot-time `warn`.
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[],
        };
        let resolver = orgs_resolver_desc();
        let leftover =
            unreachable_resolvers(&[&app], &[TypeId::of::<AppMod>()], &[&resolver]);
        assert_eq!(leftover, vec!["OrgsResolver"]);
    }

    #[test]
    fn resolver_listed_only_in_unreachable_module_is_unreachable() {
        // Listed, but in a module the root does not import — module-gating skips
        // it (its `TypeId` is not in the reachable provider set).
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "OrgsResolver",
                provides: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
            }],
        };
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[], // does not import BillingModule
            providers: &[],
        };
        let keys = reachable_provider_ids(&[&app, &billing], &[TypeId::of::<AppMod>()], &HashSet::new());
        assert!(!keys.contains(&TypeId::of::<OrgsResolver>()));
        let resolver = orgs_resolver_desc();
        let leftover = unreachable_resolvers(
            &[&app, &billing],
            &[TypeId::of::<AppMod>()],
            &[&resolver],
        );
        assert_eq!(leftover, vec!["OrgsResolver"]);
    }

    #[test]
    fn global_keys_are_reachable() {
        // Seeds + factory outputs land in the global set and must appear in
        // `reachable_provider_ids` so a transport that filters by membership
        // does not drop providers that depend on them (or themselves).
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[],
        };
        let keys = reachable_provider_ids(&[&app], &[TypeId::of::<AppMod>()], &global());
        assert!(keys.contains(&TypeId::of::<Db>()));
    }
}
