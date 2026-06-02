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
//! registry, so — unlike a provider, which is reached only when something injects
//! it — *every* resolver linked into the binary is live. It used to belong to no
//! module, leaving its `#[inject]` services outside any import closure to check
//! against. It is now brought under the contract the same way a controller is:
//! **declare it in a module's `providers = [...]`** (the resolver-membership
//! decision). `#[resolver]` emits an `impl Discoverable` (a no-op `register` —
//! the schema still builds it from the assembled container — and an `injected`
//! reporting its `#[inject]` keys, its `#[use_guards]` resolver/operation guards,
//! and the container-resolved `&Service` dependencies of its `#[field]`s), so a
//! listed resolver produces a [`ProviderDescriptor`] like any other and the graph
//! check above governs it. To keep the contract *total* rather than opt-in, the
//! macro also submits a [`ResolverDescriptor`] per resolver, and the boot fails
//! with a [`ResolverMembershipError`] if a linked resolver (hence one already in
//! the schema) is not listed in any reachable module. (A `#[field]`'s
//! `&DataLoader<…>` is request-scoped — read from the GraphQL context, not the
//! container — so it is not an injected key and stays out of the graph, like the
//! dataloaders themselves.)
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

/// A `#[resolver]` is linked into the binary (so it is part of the GraphQL
/// schema) but is not listed in the `providers = [...]` of any module reachable
/// from the application root, so its injected dependencies escape the access
/// contract. Raised at boot by the resolver-membership validation.
#[derive(Debug, Error)]
#[error(
    "resolver `{resolver}` is part of the GraphQL schema but is not declared in any module \
     reachable from the application root. Add `{resolver}` to its feature module's \
     `#[module(providers = [...])]` (like a controller) so its injected dependencies are \
     checked by the access contract."
)]
pub struct ResolverMembershipError {
    /// The resolver missing from every reachable module's `providers`.
    pub resolver: &'static str,
}

/// Marker the schema-composing layer registers when an app actually composes the
/// resolver schema, so the boot knows the link-time [`ResolverDescriptor`]
/// registry is live here. Resolver membership only matters then: a `#[resolver]`
/// is part of *a* schema only when one is composed, so an app that links resolvers
/// transitively (e.g. through a shared library) but composes no schema must not be
/// forced to declare them. The surface crate that builds the schema provides this
/// (`builder.provide(ResolverSchemaActive)`); the boot runs
/// [`validate_resolver_membership`] only when it is present.
pub struct ResolverSchemaActive;

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

/// Verify every linked resolver is a member of a module reachable from `roots`
/// (listed in its `providers = [...]`). A resolver belongs to no module on its
/// own yet is always live in the schema, so membership is what gives its
/// injected dependencies an import closure for [`validate_access_graph`] to
/// check. Pure over its inputs, like [`validate_access_graph`].
///
/// - `descriptors` — every module descriptor in the binary.
/// - `roots` — the application's root module `TypeId`(s).
/// - `resolvers` — every resolver descriptor in the binary.
pub fn validate_resolver_membership(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    resolvers: &[&ResolverDescriptor],
) -> Result<(), ResolverMembershipError> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();

    // Every provider key across the modules reachable from the roots — a
    // listed resolver provides its own `TypeId`, so its presence here is the
    // membership we require.
    let mut reachable_keys = HashSet::new();
    for module_id in reachable(roots, &by_id) {
        if let Some(desc) = by_id.get(&module_id) {
            for p in desc.providers {
                reachable_keys.insert((p.provides)());
            }
        }
    }

    for r in resolvers {
        if !reachable_keys.contains(&(r.resolver)()) {
            return Err(ResolverMembershipError { resolver: r.name });
        }
    }
    Ok(())
}

/// Validate the link-time module registry against the app's roots and global
/// set. Called by [`App`](crate::App) at boot, alongside
/// [`validate_resolver_membership_from_inventory`]. Kept returning the concrete
/// [`AccessGraphError`] (rather than a unified enum) so a caller can `downcast`
/// the boot failure to the precise cause.
pub(crate) fn validate_from_inventory(
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> Result<(), AccessGraphError> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    validate_access_graph(&descriptors, roots, global)
}

/// Validate resolver membership against the link-time registry: every linked
/// resolver must be listed in a module reachable from `roots`. Called by
/// [`App`](crate::App) at boot, after [`validate_from_inventory`].
pub(crate) fn validate_resolver_membership_from_inventory(
    roots: &[TypeId],
) -> Result<(), ResolverMembershipError> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    let resolvers: Vec<&ResolverDescriptor> = inventory::iter::<ResolverDescriptor>().collect();
    validate_resolver_membership(&descriptors, roots, &resolvers)
}

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
    fn listed_resolver_passes_membership() {
        // A resolver listed in `providers` is a member of its reachable module.
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
        validate_resolver_membership(&[&app], &[TypeId::of::<AppMod>()], &[&orgs_resolver_desc()])
            .expect("a resolver listed in a reachable module's providers is a member");
    }

    #[test]
    fn unlisted_resolver_fails_membership() {
        // The resolver is linked (hence in the schema) but listed in no module.
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[],
        };
        let err = validate_resolver_membership(
            &[&app],
            &[TypeId::of::<AppMod>()],
            &[&orgs_resolver_desc()],
        )
        .expect_err("a resolver in no reachable module must fail the boot");
        assert_eq!(err.resolver, "OrgsResolver");
        assert!(err.to_string().contains("OrgsResolver"), "{err}");
    }

    #[test]
    fn resolver_listed_only_in_unreachable_module_fails_membership() {
        // Listed, but in a module the root does not import — it is still live in
        // the schema (the registry is global), so membership must still fail.
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
        let err = validate_resolver_membership(
            &[&app, &billing],
            &[TypeId::of::<AppMod>()],
            &[&orgs_resolver_desc()],
        )
        .expect_err("a resolver listed only in an unimported module must fail");
        assert_eq!(err.resolver, "OrgsResolver");
    }
}
