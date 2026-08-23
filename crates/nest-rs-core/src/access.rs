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
//! A self-composing item joins the contract the same way — through module
//! membership. Something listed in some reachable module's `providers = [...]`
//! is governed like any other provider; something in no reachable module is
//! outside the app, which [`ReachableProviders`] is what tells a transport.
//! Whether that absence is worth a word, and in what sentence, belongs to the
//! transport: this module answers *what is reachable* and nothing about any
//! particular edge's registry.
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
///
/// **Internal ABI.** Constructed by struct-literal in framework-macro output;
/// the `nest-rs-*-macros` crates ship in lockstep with `nest-rs-core`, so a
/// new field is added to the emitter and this struct in the same release and
/// never breaks downstream (which never hand-constructs it). Do not construct
/// it by hand.
#[doc(hidden)]
pub struct ProviderDescriptor {
    /// The provider type's name, used to name the offending consumer in a boot
    /// access error.
    pub name: &'static str,
    /// The container key this provider registers under:
    /// `TypeId::of::<Concrete>()` for an `#[injectable]`, or
    /// `TypeId::of::<Arc<dyn Trait>>()` for a `Foo as dyn Trait` binding.
    pub provides: fn() -> TypeId,
    /// Extra container keys this provider registers on its module's behalf,
    /// each with the label a boot error names it by. See
    /// [`Discoverable::also_provides`](crate::Discoverable::also_provides) —
    /// almost always empty.
    pub also_provides: fn() -> Vec<(TypeId, &'static str)>,
    /// `TypeId` of each bare `#[inject]` field plus each attribute-referenced
    /// layer (`#[use_guards]` / `#[use_filters]` / `#[use_interceptors]`).
    pub injects: fn() -> Vec<TypeId>,
    /// Human-readable label for each [`injects`](Self::injects) entry, in the
    /// same order, so a dependency no module provides is named in the boot
    /// error. May be shorter than `injects` (a provider that emits no names
    /// falls back to a placeholder); never longer.
    pub inject_names: fn() -> Vec<&'static str>,
    /// Each **keyed** `#[inject(key = "…")]` field, validated against the
    /// global keyed set. Empty for providers with no keyed dependency.
    pub injects_keyed: fn() -> Vec<KeyedDependency>,
}

/// Per-module descriptor submitted to the link-time registry by `#[module]`.
///
/// **Internal ABI** (see [`ProviderDescriptor`]) — macro-constructed, lockstep
/// with `nest-rs-core`; do not hand-construct.
#[doc(hidden)]
pub struct ModuleDescriptor {
    /// `TypeId` of the `#[module]` struct — the graph node's identity, matched
    /// against other modules' [`imports`](Self::imports).
    pub module: fn() -> TypeId,
    /// The module type's name, used to name the module in a boot access error.
    pub name: &'static str,
    /// Statically-typed imports only. Dynamic (`for_root(...)`) imports
    /// contribute only global infrastructure, never an injectable a provider
    /// could depend on.
    pub imports: &'static [fn() -> TypeId],
    /// Every provider this module declares in its `providers = [...]`, each with
    /// the dependency information the access-graph walk needs.
    pub providers: &'static [ProviderDescriptor],
}

inventory::collect!(ModuleDescriptor);

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
    /// Module that owns the offending consumer and whose imports fall short.
    pub module: &'static str,
    /// Provider that reached for a dependency its module cannot see.
    pub consumer: &'static str,
    /// The dependency that was out of reach.
    pub dependency: &'static str,
    /// Module that actually provides `dependency` — the one to import to fix it.
    pub owner: &'static str,
}

/// A provider depends on something **no module provides** — not global
/// infrastructure, not in its import closure, not registered anywhere. Raised at
/// boot so a lazily-built scoped/transient provider fails cleanly here instead
/// of panicking at its first `get(...).expect(...)` resolution. An *eager*
/// provider's missing dependency lands here too: the register phase defers it
/// to this check rather than panicking ahead of it, so every wiring failure is
/// one `Result`.
#[derive(Debug, Error)]
#[error(
    "unmet dependency: `{consumer}` (in module `{module}`) depends on `{dependency}`, but no \
     module provides it and it is not global infrastructure (a seed or factory output). Add a \
     provider for `{dependency}` to a module reachable from the root, or seed it at \
     `App::builder()`."
)]
pub struct MissingDependencyError {
    /// Module that owns the consumer whose dependency is unmet.
    pub module: &'static str,
    /// Provider whose dependency no module supplies.
    pub consumer: &'static str,
    /// The dependency that is registered nowhere and is not global infra.
    pub dependency: &'static str,
}

/// A **singleton** provider `#[inject]`s one the container never places in the
/// singleton map — a `#[injectable(scope = request)]` or `scope = transient`
/// provider.
///
/// Raised at boot because the container cannot honour it and, before this error,
/// did not say so: the register phase gates readiness on the singleton map, so
/// such a provider never became ready, was classified unprovided, and was
/// **dropped** — with everything downstream of it — while the boot returned
/// `Ok` and emitted nothing. The symptom surfaced far away, as a service
/// missing at its first `Container::get`, or as an inert-host `warn` offering
/// five causes none of which was this one.
///
/// **The remedy names the concept, never the edge crates**, and that is the
/// same law [`target`](crate::target) and [`operation_log`](crate::operation_log)
/// state for themselves: the kernel holds no name for a concern it does not know
/// exists. It listed three `Scoped<T>` paths for one round — `nest_rs_http`,
/// `nest_rs_graphql`, `nest_rs_mcp` — copied from a prose list in
/// `framework.md` that was itself three of four, so a developer who hit this on
/// a WS gateway was handed three paths none of which was theirs while
/// `nest_rs_ws::Scoped<T>` existed. Nothing compiles against a message, so the
/// fourth would never have been added; every future edge would have inherited
/// the same wrong remedy.
///
/// **The reason is worded per arm, because the two arms are not the same fact.**
/// A request-scoped provider genuinely has no instance outside a request. A
/// transient one does — `Container::get` opens a throwaway scope and builds it
/// ([`Discoverable`](crate::Discoverable)'s own table says so). What is true of
/// both, and is what this check reads, is that neither is ever in the singleton
/// map the register phase gates readiness on.
#[derive(Debug, Error)]
#[error(
    "scope violation: `{consumer}` (in module `{module}`) is a singleton and injects \
     `{dependency}`, which is request-scoped or transient. Neither is ever placed in \
     the singleton map a singleton's dependencies are resolved from, so there is \
     nothing for `{consumer}` to hold once at boot. Reach it through the request \
     boundary of the edge that dispatches the work — the `Scoped<T>` its crate \
     exports — or make `{consumer}` request-scoped too."
)]
pub struct ScopeViolationError {
    /// Module that owns the offending consumer.
    pub module: &'static str,
    /// The singleton provider whose `#[inject]` cannot be honoured.
    pub consumer: &'static str,
    /// The request-scoped or transient dependency it named.
    pub dependency: &'static str,
}

/// The failure modes of the bare (non-keyed) access-graph pass: a cross-module
/// reach that no import covers, or a dependency no module provides.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AccessError {
    /// A provider reached across modules for something no import covers.
    #[error(transparent)]
    CrossModule(#[from] AccessGraphError),
    /// A provider depends on something no module provides at all.
    #[error(transparent)]
    Missing(#[from] MissingDependencyError),
    /// A singleton injected a provider that only exists inside a request.
    #[error(transparent)]
    Scope(#[from] ScopeViolationError),
}

impl AccessError {
    /// Flatten into an `anyhow::Error` carrying the **concrete** inner error,
    /// discarding the enum wrapper, so a boot failure downcasts to
    /// `AccessGraphError` / `MissingDependencyError` directly — the wrapper is an
    /// internal detail of the pass, not part of the boot-error contract.
    /// `anyhow::Error::new` (over the concrete type) is what preserves the
    /// downcast; boxing to `dyn Error` first would lose it.
    pub fn into_anyhow(self) -> anyhow::Error {
        match self {
            AccessError::CrossModule(e) => anyhow::Error::new(e),
            AccessError::Missing(e) => anyhow::Error::new(e),
            AccessError::Scope(e) => anyhow::Error::new(e),
        }
    }
}

/// A concrete or keyed provider was registered more than once — two modules,
/// or a seed and a module, providing the same type. Raised at boot rather than
/// silently last-write-wins, uniform with every other wiring error.
/// Trait-object bindings (`provide_dyn`) and the test override path are exempt
/// (they are the *intended* replacement mechanisms).
#[derive(Debug, Error)]
#[error(
    "duplicate provider: `{type_name}` is registered more than once. Two modules (or a seed and a \
     module) provide the same type — remove the redundant registration, or expose it as a \
     `dyn Trait` binding if a deliberate override was intended."
)]
pub struct DuplicateProviderError {
    /// The type registered more than once.
    pub type_name: &'static str,
}

/// Two import sites each *declared* a value for the same type, and one of them
/// would have to lose. Raised by `AppBuilder::build` before any factory runs.
///
/// The framework refuses to pick because both call sites are deliberate: the
/// container resolves an ordinary collision by keeping the first factory
/// queued, which would make the surviving value a function of `imports = [..]`
/// order — silently dropped, on the wrong side of *no silent failure*. Only a
/// **declaration** contests (`ContainerBuilder::provide_declared_factory`): a
/// pinned config base, or a module binding an implementation a sibling module
/// also binds. A module queuing the same default twice never does, so a diamond
/// import stays legal.
///
/// `remedy` comes from the declaring call site, which is the only place that
/// knows what the two sites were.
#[derive(Debug, Error)]
#[error("contested declaration: `{type_name}` is declared by two import sites. {remedy}")]
pub struct ContestedDeclarationError {
    /// The type declared more than once.
    pub type_name: &'static str,
    /// What the reader should do instead, supplied by the declaring seam.
    pub remedy: &'static str,
}

/// A module queued an async factory, but the boot went through the synchronous
/// [`App::new`](crate::App::new), which has no factory phase to drain it.
///
/// The value would simply never exist: a `Module::for_root(cfg)` whose config
/// resolves to nothing, a pool nobody opened. Injecting it fails the access
/// graph, but reading it through `Container::get` would just return `None` —
/// so the boot refuses instead of leaving the hole open.
#[derive(Debug, Error)]
#[error(
    "`{type_name}` is provided by an async factory, which the synchronous `App::new` never runs. \
     A module's `for_root(..)` and `ConfigModule::for_feature` both queue one. Boot with \
     `App::builder().module::<M>().build().await` instead."
)]
pub struct UnresolvedFactoryError {
    /// The type whose factory nothing would drain.
    pub type_name: &'static str,
}

/// Every factory left in the queue waits on a factory output still in it — one
/// another's, or its own — so none can run first: a cycle in the `*_after`
/// declarations. Raised by `AppBuilder::build` instead of running them in queue
/// order, which would hand one of them a snapshot missing what it declared it
/// reads.
#[derive(Debug, Error)]
#[error(
    "factory cycle: {type_names:?} — each waits on a factory output that a member \
     of this set (itself included) would provide, so no order satisfies them. \
     Drop one `_after` declaration."
)]
pub struct FactoryCycleError {
    /// The types whose factories wait on each other.
    pub type_names: Vec<&'static str>,
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
    /// Module that owns the consumer with the unreachable keyed dependency.
    pub module: &'static str,
    /// Provider whose `#[inject(key = "…")]` has no keyed provider registered.
    pub consumer: &'static str,
    /// The injected type of the keyed dependency.
    pub type_name: &'static str,
    /// The requested key — named alongside the type so both appear in the error.
    pub key: &'static str,
}

/// Provider keys reachable from the app's module tree, seeded into the
/// container so transports can module-gate their inventory: a `#[resolver]`
/// linked into the binary but living in no reachable module is silently
/// skipped from the GraphQL schema instead of failing the boot.
///
/// Includes every provider declared in a reachable module's
/// `providers = [...]` plus the global infrastructure keys.
pub struct ReachableProviders(pub std::collections::HashSet<TypeId>);

/// The reachable providers again, this time **in declaration order** — modules
/// depth-first from the root along `imports = [...]`, each module's
/// `providers = [...]` left to right.
///
/// Seeded beside [`ReachableProviders`] for the one discovery seam that
/// promises an order the developer wrote: the event bus dispatches listeners in
/// "the order their providers appear in `providers = [...]`, then the order
/// their methods appear in the `#[listeners]` block". Link order cannot deliver
/// that — it is stable per binary but reshuffles when the code changes, which
/// is the worst of both worlds: two listeners ordered deliberately and verified
/// locally get silently rearranged the next time somebody adds a third.
pub struct ProviderOrder(HashMap<TypeId, usize>);

impl ProviderOrder {
    /// Index each provider by its position in `order`. Stored as a map rather
    /// than the `Vec` it comes from because the only consumer asks for a
    /// **rank** inside a sort key — a linear scan there is one pass per
    /// comparison, and the walk that produced the order already visited every
    /// provider once.
    pub fn new(order: impl IntoIterator<Item = TypeId>) -> Self {
        Self(
            order
                .into_iter()
                .enumerate()
                .map(|(i, id)| (id, i))
                .collect(),
        )
    }

    /// Rank of `provider` in declaration order; unreachable providers sort last
    /// (they are skipped before dispatch anyway).
    pub fn rank(&self, provider: TypeId) -> usize {
        self.0.get(&provider).copied().unwrap_or(usize::MAX)
    }
}

/// Validate the access graph: every provider's dependency must be reachable
/// from its module's import closure or be global infrastructure.
///
/// Pure over its inputs (no link-time registry access). `roots` without a
/// descriptor terminate a branch, making a hand-written root a no-op.
pub(crate) fn validate_access_graph(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    global: &HashSet<TypeId>,
    registered: &HashSet<TypeId>,
    scoped_or_transient: &HashSet<TypeId>,
) -> Result<(), AccessError> {
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
            // A key the provider installs for its module — named by the key's own
            // label, not the provider's, so the error reads
            // "`WsServer<NotifyNs>` is provided by `WsModule`" rather than naming
            // the plumbing that installed it.
            for (key, label) in (p.also_provides)() {
                provided_by.entry(key).or_insert((label, d.name));
            }
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
                    closure_keys.extend((p.also_provides)().into_iter().map(|(key, _)| key));
                }
            }
        }

        for p in desc.providers {
            let deps = (p.injects)();
            let names = (p.inject_names)();
            // A singleton may not inject a provider that only exists inside a
            // request. `framework.md` states the rule — "**One level deep**:
            // request-scoped may inject singletons; never the reverse" — and
            // nothing enforced it, so the case failed **silently**: the register
            // phase gates readiness on the singleton map alone, so a singleton
            // whose dependency is a scoped or transient factory never becomes
            // ready, is classified unprovided, and is dropped along with
            // everything downstream of it. The boot returned `Ok`, emitted
            // nothing, and the provider was simply absent at first `get`.
            //
            // Checked before the presence checks below, because the dependency
            // *is* declared and reachable — presence was never the problem.
            let consumer_is_scoped = scoped_or_transient.contains(&(p.provides)());
            if !consumer_is_scoped {
                for (i, dep) in deps.iter().enumerate() {
                    if scoped_or_transient.contains(dep) {
                        return Err(ScopeViolationError {
                            module: desc.name,
                            consumer: p.name,
                            dependency: names.get(i).copied().unwrap_or("<unnamed dependency>"),
                        }
                        .into());
                    }
                }
            }
            for (i, dep) in deps.iter().enumerate() {
                if global.contains(dep) || closure_keys.contains(dep) {
                    continue;
                }
                // Provided by some other module but not imported ⇒ a cross-module
                // reach; provided by no module at all ⇒ an unmet dependency that
                // would otherwise panic at first resolution (lazy) or at the
                // register phase (eager). The dependency name is index-aligned
                // with `injects`; a provider that emits no names falls back.
                if let Some((dependency, owner)) = provided_by.get(dep) {
                    return Err(AccessGraphError {
                        module: desc.name,
                        consumer: p.name,
                        dependency,
                        owner,
                    }
                    .into());
                }
                // Not a declarative provider anywhere. It may still be resolvable
                // — a hand-written `impl Module` (e.g. `EventsModule`) or a lazy
                // factory registers imperatively, invisible to this graph. Only
                // when it is absent from the actual registered set is it a
                // genuinely unmet dependency that would panic at resolution.
                if registered.contains(dep) {
                    continue;
                }
                return Err(MissingDependencyError {
                    module: desc.name,
                    consumer: p.name,
                    dependency: names.get(i).copied().unwrap_or("<unnamed dependency>"),
                }
                .into());
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
pub(crate) fn validate_keyed_access_graph(
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
    // The ordered walk, collected — order is irrelevant to a set, and one
    // traversal is one thing to keep correct.
    reachable_in_order(roots, by_id).into_iter().collect()
}

/// The same walk, **in source order**: roots first, then each module's
/// `imports = [...]` left to right, depth-first, first occurrence winning.
///
/// A `HashSet` cannot answer "which provider was declared first", and link
/// order cannot either — `inventory` hands entries back in whatever order the
/// linker emitted them, which is stable per binary and changes when the code
/// does. Any discovery seam that promises a *declaration* order (the event bus
/// does) needs an ordering derived from the module graph instead.
fn reachable_in_order(roots: &[TypeId], by_id: &HashMap<TypeId, &ModuleDescriptor>) -> Vec<TypeId> {
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    let mut stack: Vec<TypeId> = roots.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        order.push(id);
        if let Some(desc) = by_id.get(&id) {
            // Reversed onto the stack so the first import is visited first.
            for import in desc.imports.iter().rev() {
                stack.push((import)());
            }
        }
    }
    order
}

/// Provider keys reachable from `roots` via the module import graph plus
/// `global`. Used at boot to seed [`ReachableProviders`] so transports can
/// module-gate their discovery. Pure over its inputs.
pub(crate) fn reachable_provider_ids(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    global: &HashSet<TypeId>,
) -> HashSet<TypeId> {
    // The same providers [`provider_order`] enumerates, plus global infra — one
    // walk, one collection loop, two shapes of the answer.
    let mut keys = global.clone();
    keys.extend(provider_order(descriptors, roots));
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

/// Every reachable provider in **declaration order**: modules depth-first from
/// the roots along their `imports = [...]`, and within each module its
/// `providers = [...]` left to right. First occurrence wins, so a diamond
/// import ranks a provider where it is first reached. Pure over its inputs.
pub(crate) fn provider_order(descriptors: &[&ModuleDescriptor], roots: &[TypeId]) -> Vec<TypeId> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    for module_id in reachable_in_order(roots, &by_id) {
        let Some(desc) = by_id.get(&module_id) else {
            continue;
        };
        for p in desc.providers {
            let key = (p.provides)();
            if seen.insert(key) {
                order.push(key);
            }
        }
    }
    order
}

/// Boot-time equivalent of [`provider_order`] against the link-time registry.
pub(crate) fn provider_order_from_inventory(roots: &[TypeId]) -> Vec<TypeId> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    provider_order(&descriptors, roots)
}

/// Boot-time entry point: validate the link-time module registry against the
/// app's roots and global set. Returns the concrete [`AccessGraphError`] so a
/// caller can downcast the boot failure to the precise cause.
pub(crate) fn validate_from_inventory(
    roots: &[TypeId],
    global: &HashSet<TypeId>,
    registered: &HashSet<TypeId>,
    scoped_or_transient: &HashSet<TypeId>,
) -> Result<(), AccessError> {
    let descriptors: Vec<&ModuleDescriptor> = inventory::iter::<ModuleDescriptor>().collect();
    validate_access_graph(&descriptors, roots, global, registered, scoped_or_transient)
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

    fn no_names() -> Vec<&'static str> {
        Vec::new()
    }

    fn billing_names() -> Vec<&'static str> {
        vec!["UsersService"]
    }

    fn no_keyed_deps() -> Vec<KeyedDependency> {
        Vec::new()
    }

    /// The default for every provider that registers only itself.
    fn provides_only_itself() -> Vec<(TypeId, &'static str)> {
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
                inject_names: no_names,
                injects_keyed: no_keyed_deps,
                also_provides: provides_only_itself,
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
        validate_access_graph(
            &descriptors,
            &[TypeId::of::<AppMod>()],
            &global(),
            &HashSet::new(),
            &HashSet::new(),
        )
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
                    name: "AuthzAbility",
                    provides: || TypeId::of::<UsersService>(),
                    injects: no_deps,
                    inject_names: no_names,
                    injects_keyed: no_keyed_deps,
                    also_provides: provides_only_itself,
                },
                ProviderDescriptor {
                    name: "AppGuard",
                    provides: || TypeId::of::<AppGuard>(),
                    injects: billing_deps,
                    inject_names: no_names,
                    injects_keyed: no_keyed_deps,
                    also_provides: provides_only_itself,
                },
            ],
        };
        validate_access_graph(
            &[&app],
            &[TypeId::of::<AppMod>()],
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
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
                inject_names: no_names,
                injects_keyed: no_keyed_deps,
                also_provides: provides_only_itself,
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
            &HashSet::new(),
            &HashSet::new(),
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
                inject_names: no_names,
                injects_keyed: no_keyed_deps,
                also_provides: provides_only_itself,
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
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect_err("reaching an unimported module must fail");

        let AccessError::CrossModule(err) = err else {
            panic!("a cross-module reach must be a CrossModule error, got {err:?}");
        };
        assert_eq!(err.consumer, "BillingService");
        assert_eq!(err.module, "BillingModule");
        assert_eq!(err.dependency, "UsersService");
        assert_eq!(err.owner, "UsersModule");
        let msg = err.to_string();
        assert!(msg.contains("BillingService"), "{msg}");
        assert!(msg.contains("UsersModule"), "{msg}");
    }

    #[test]
    fn a_dependency_no_module_provides_is_a_named_boot_error() {
        // `BillingService` depends on `UsersService`, but no module provides it
        // (the users module is absent) and it is not global. This is the case
        // that used to slip past the graph and panic at first `get()` for a lazy
        // provider — now a clean boot error naming both provider and dependency.
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                inject_names: billing_names,
                injects_keyed: no_keyed_deps,
                also_provides: provides_only_itself,
            }],
        };
        let err = validate_access_graph(
            &[&billing],
            &[TypeId::of::<BillingMod>()],
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect_err("a dependency no module provides must fail the boot");

        let AccessError::Missing(err) = err else {
            panic!("an unmet dependency must be a Missing error, got {err:?}");
        };
        assert_eq!(err.consumer, "BillingService");
        assert_eq!(err.module, "BillingModule");
        assert_eq!(err.dependency, "UsersService");
        let msg = err.to_string();
        assert!(msg.contains("BillingService"), "{msg}");
        assert!(msg.contains("UsersService"), "{msg}");
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
                inject_names: no_names,
                injects_keyed: no_keyed_deps,
                also_provides: provides_only_itself,
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
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect("a module outside the root's import tree is not validated");
    }

    #[test]
    fn hand_written_root_without_descriptor_is_a_noop() {
        validate_access_graph(
            &[],
            &[TypeId::of::<AppMod>()],
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect("a root with no descriptor validates trivially");
    }

    #[test]
    fn a_listed_provider_is_reachable() {
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "OrgsResolver",
                provides: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
                inject_names: no_names,
                injects_keyed: no_keyed_deps,
                also_provides: provides_only_itself,
            }],
        };
        let keys = reachable_provider_ids(&[&app], &[TypeId::of::<AppMod>()], &HashSet::new());
        assert!(keys.contains(&TypeId::of::<OrgsResolver>()));
    }

    #[test]
    fn a_provider_listed_only_in_an_unreachable_module_is_not_reachable() {
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "OrgsResolver",
                provides: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
                inject_names: no_names,
                injects_keyed: no_keyed_deps,
                also_provides: provides_only_itself,
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

    // A stand-in for the keyed `OAuthClient` case: one concrete type injected
    // twice, disambiguated by key.
    struct OAuthClient;

    fn github_dep() -> Vec<KeyedDependency> {
        vec![KeyedDependency {
            key: ProviderKey::named::<OAuthClient>("github"),
            type_name: "OAuthClient",
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
                inject_names: no_names,
                injects_keyed: github_dep,
                also_provides: provides_only_itself,
            }],
        }
    }

    #[test]
    fn keyed_dependency_supplied_globally_passes() {
        let users = keyed_consumer_module();
        let global_keyed = HashSet::from([ProviderKey::named::<OAuthClient>("github")]);
        validate_keyed_access_graph(&[&users], &[TypeId::of::<UsersMod>()], &global_keyed)
            .expect("a globally-seeded keyed provider satisfies the keyed dependency");
    }

    #[test]
    fn unmet_keyed_dependency_is_rejected_naming_type_and_key() {
        let users = keyed_consumer_module();
        let err =
            validate_keyed_access_graph(&[&users], &[TypeId::of::<UsersMod>()], &HashSet::new())
                .expect_err("a keyed dependency with no keyed provider must fail");
        assert_eq!(err.consumer, "SocialLoginService");
        assert_eq!(err.module, "UsersModule");
        assert_eq!(err.type_name, "OAuthClient");
        assert_eq!(err.key, "github");
        let msg = err.to_string();
        assert!(msg.contains("OAuthClient"), "names the type: {msg}");
        assert!(msg.contains("github"), "names the key: {msg}");
    }

    #[test]
    fn wrong_key_does_not_satisfy_a_keyed_dependency() {
        // Only the exact `(type, key)` counts — a different key of the same
        // type leaves the dependency unmet.
        let users = keyed_consumer_module();
        let global_keyed = HashSet::from([ProviderKey::named::<OAuthClient>("google")]);
        let err =
            validate_keyed_access_graph(&[&users], &[TypeId::of::<UsersMod>()], &global_keyed)
                .expect_err("the `google` key must not satisfy a `github` dependency");
        assert_eq!(err.key, "github");
    }
}
