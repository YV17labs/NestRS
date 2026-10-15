//! Build-time validation of the module import graph (the access contract).
//!
//! A provider's `#[inject]` dependency (and its `#[use_guards]` /
//! `#[use_filters]` / `#[use_interceptors]` layers, and each provider its
//! handlers resolve through a parameter) must be provided by its own
//! module, a module in its transitive import closure, or global infrastructure
//! (seeds + factory outputs); otherwise the boot fails with an
//! [`AccessGraphError`]. Runtime [`Container::get`](crate::Container::get) is
//! an unchecked escape hatch.

use std::any::TypeId;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::container::{ProviderKey, ProviderScope};
use crate::error::{
    AccessError, AccessGraphError, KeyedDependencyError, MissingDependencyError,
    ProviderCycleError, ScopeViolationError, UnbuiltCause,
};

pub(crate) use self::__private::{ModuleDescriptor, ProviderDescriptor};

/// How a boot error names a dependency whose provider reported no label.
pub(crate) const UNNAMED_DEPENDENCY: &str = "<unnamed dependency>";

/// `access` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use std::any::TypeId;

    use crate::container::KeyedDependency;

    /// One provider declared in a module's `providers = [...]`, recorded by the
    /// `#[module]` macro for the access-graph check.
    pub struct ProviderDescriptor {
        /// The provider type's name, for boot errors.
        pub name: &'static str,
        /// The container key this provider registers under:
        /// `TypeId::of::<Concrete>()` for an `#[injectable]`, or
        /// `TypeId::of::<Arc<dyn Trait>>()` for a `Foo as dyn Trait` binding.
        pub provides: fn() -> TypeId,
        /// The provider's own type, whichever key it registers under — what tells
        /// a decorated host bound as `dyn Trait` from one no module lists.
        pub provider: fn() -> TypeId,
        /// Extra container keys this provider registers on its module's behalf,
        /// each with its label ([`Discoverable::also_provides`](crate::Discoverable::also_provides)).
        pub also_provides: fn() -> Vec<(TypeId, &'static str)>,
        /// `TypeId` of each bare `#[inject]` field plus each attribute-referenced
        /// layer (`#[use_guards]` / `#[use_filters]` / `#[use_interceptors]`).
        pub injects: fn() -> Vec<TypeId>,
        /// Label for each [`injects`](Self::injects) entry, in the same order; may
        /// be shorter than `injects`, never longer.
        pub inject_names: fn() -> Vec<&'static str>,
        /// `TypeId` of each `#[inject] Option<Arc<…>>` field, checked like
        /// [`injects`](Self::injects) but for one no reached module provides,
        /// which is simply absent; a [`Net`](crate::Net) around the provider
        /// follows it too.
        pub injects_optional: fn() -> Vec<TypeId>,
        /// Label for each [`injects_optional`](Self::injects_optional) entry, in
        /// the same order; may be shorter, never longer.
        pub inject_optional_names: fn() -> Vec<&'static str>,
        /// Each **keyed** `#[inject(key = "…")]` field, validated against the
        /// global keyed set.
        pub injects_keyed: fn() -> Vec<KeyedDependency>,
        /// Each provider a handler resolves through a parameter, with its label
        /// ([`Discoverable::injected_per_request`](crate::Discoverable::injected_per_request)):
        /// checked like [`injects`](Self::injects), never for scope.
        pub injects_per_request: fn() -> Vec<(TypeId, &'static str)>,
    }

    /// Per-module descriptor submitted to the link-time registry by `#[module]`.
    pub struct ModuleDescriptor {
        /// `TypeId` of the `#[module]` struct, matched against other modules'
        /// [`imports`](Self::imports).
        pub module: fn() -> TypeId,
        /// The module type's name, for boot errors.
        pub name: &'static str,
        /// Every import in declaration order: a module by its type, a dynamic
        /// import (`for_root(...)`) by the module its value declares
        /// ([`DynamicModule::module`](crate::DynamicModule::module)).
        pub imports: &'static [fn() -> TypeId],
        /// Every provider this module declares in its `providers = [...]`.
        pub providers: &'static [ProviderDescriptor],
    }
}

inventory::collect!(ModuleDescriptor);

/// Provider keys reachable from the app's module tree — every provider of a
/// reachable module plus the global infrastructure keys — seeded into the
/// container so transports can module-gate their inventory.
pub struct ReachableProviders(pub std::collections::HashSet<TypeId>);

impl ReachableProviders {
    /// Whether `provider` is reachable — the gate every inventory-draining edge
    /// reads before it mounts an entry. No set means the access graph never ran
    /// (a hand-built container in a test), so everything is in scope.
    pub fn reaches(set: Option<&Self>, provider: TypeId) -> bool {
        set.is_none_or(|r| r.0.contains(&provider))
    }
}

/// What a discovery site reads to say why a host it found is inert: the crates
/// the app's roots are written in, and the modules those roots reach
/// ([`inert_host`](crate::inert_host)).
pub struct Composition {
    root_crates: Vec<&'static str>,
    modules: HashSet<TypeId>,
    overridden: HashSet<TypeId>,
}

impl Composition {
    /// The composition of an app built from `roots`, each with its type's
    /// name, in which a test's overrides stand in for the providers
    /// `overridden`.
    pub(crate) fn from_descriptors(
        descriptors: &[&ModuleDescriptor],
        roots: &[(TypeId, &'static str)],
        overridden: HashSet<TypeId>,
    ) -> Self {
        let by_id: HashMap<TypeId, &ModuleDescriptor> =
            descriptors.iter().map(|d| ((d.module)(), *d)).collect();
        let ids: Vec<TypeId> = roots.iter().map(|(id, _)| *id).collect();
        let mut root_crates: Vec<&'static str> =
            roots.iter().map(|(_, name)| crate_of(name)).collect();
        root_crates.sort_unstable();
        root_crates.dedup();
        Self {
            root_crates,
            modules: reachable(&ids, &by_id),
            overridden,
        }
    }

    /// Whether `origin` — a `module_path!()` — is in a crate the app's roots
    /// are written in.
    pub fn is_the_apps_own(&self, origin: &str) -> bool {
        self.root_crates.contains(&crate_of(origin))
    }

    /// Whether the app reaches the module `module`.
    pub fn reaches(&self, module: TypeId) -> bool {
        self.modules.contains(&module)
    }

    /// Whether a test's override stands in for the provider `provider`, which
    /// the app then never built.
    pub(crate) fn overrides(&self, provider: TypeId) -> bool {
        self.overridden.contains(&provider)
    }
}

/// The crate a `module_path!()` or a `type_name` is rooted in.
fn crate_of(path: &str) -> &str {
    path.split("::").next().unwrap_or(path)
}

/// The reachable providers **in declaration order** — modules depth-first from
/// the root along `imports = [...]`, each module's `providers = [...]` left to
/// right — for the event bus's listener order, which link order cannot give.
pub struct ProviderOrder(HashMap<TypeId, usize>);

impl ProviderOrder {
    /// Index each provider by its position in `order`.
    pub fn new(order: impl IntoIterator<Item = TypeId>) -> Self {
        Self(positions(order))
    }

    /// Rank of `provider` in declaration order; unreachable providers sort last.
    pub fn rank(&self, provider: TypeId) -> usize {
        self.0.get(&provider).copied().unwrap_or(usize::MAX)
    }
}

/// Each type of `order` by its position in it.
pub(crate) fn positions(order: impl IntoIterator<Item = TypeId>) -> HashMap<TypeId, usize> {
    order
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect()
}

/// Validate the access graph: every provider's dependency must be reachable
/// from its module's import closure or be global infrastructure.
///
/// Pure over its inputs (no link-time registry access). `roots` without a
/// descriptor terminate a branch, making a hand-written root a no-op. A
/// provider in `overridden` is never built, so what it injects is not checked.
pub(crate) fn validate_access_graph(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    global: &HashSet<TypeId>,
    registered: &HashSet<TypeId>,
    scoped_or_transient: &HashSet<TypeId>,
    overridden: &HashSet<TypeId>,
) -> Result<(), AccessError> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();

    let reached = reachable(roots, &by_id);
    // An optional dependency's owner is read among the modules this app reaches:
    // one only another binary's module provides is absent here, not a leak.
    let provided_by = owners(descriptors.iter().copied());
    let reached_provided_by = owners(
        descriptors
            .iter()
            .copied()
            .filter(|d| reached.contains(&(d.module)())),
    );

    for module_id in &reached {
        let Some(desc) = by_id.get(module_id) else {
            continue;
        };

        let mut closure_keys = HashSet::new();
        for import_id in reachable(&[*module_id], &by_id) {
            if let Some(imported) = by_id.get(&import_id) {
                closure_keys.extend(imported.providers.iter().flat_map(provided_keys));
            }
        }

        for p in desc
            .providers
            .iter()
            .filter(|p| !overridden.contains(&(p.provider)()))
        {
            let (injects, names) = ((p.injects)(), (p.inject_names)());
            let (optionals, optional_names) = ((p.injects_optional)(), (p.inject_optional_names)());
            let required = labelled(&injects, &names);
            let optional = labelled(&optionals, &optional_names);
            let deps: Vec<(TypeId, &'static str, bool)> = required
                .map(|(dep, name)| (dep, name, false))
                .chain(optional.map(|(dep, name)| (dep, name, true)))
                .collect();
            // Without this, the register phase (which gates readiness on the
            // singleton map) silently drops a singleton holding a scoped or
            // transient dependency, and everything downstream of it — or, held
            // optionally, builds it with `None` forever.
            let consumer_is_scoped = scoped_or_transient.contains(&(p.provides)());
            if !consumer_is_scoped
                && let Some((_, dependency, _)) = deps
                    .iter()
                    .find(|(dep, _, _)| scoped_or_transient.contains(dep))
            {
                return Err(ScopeViolationError {
                    module: desc.name,
                    consumer: p.name,
                    dependency,
                }
                .into());
            }
            // Resolved per request, so out of the scope check above: a
            // request-scoped provider is what a handler's parameter is for.
            let per_request = (p.injects_per_request)()
                .into_iter()
                .map(|(dep, name)| (dep, name, false));
            for (dep, name, optional) in deps.into_iter().chain(per_request) {
                if global.contains(&dep) || closure_keys.contains(&dep) {
                    continue;
                }
                let owners = if optional {
                    &reached_provided_by
                } else {
                    &provided_by
                };
                if let Some((dependency, owner)) = owners.get(&dep) {
                    return Err(AccessGraphError {
                        module: desc.name,
                        consumer: p.name,
                        dependency,
                        owner,
                    }
                    .into());
                }
                // A hand-written `impl Module` registers imperatively, invisible
                // to this graph; an optional nothing reached provides is absent.
                if optional || registered.contains(&dep) {
                    continue;
                }
                return Err(MissingDependencyError {
                    module: desc.name,
                    consumer: p.name,
                    dependency: name,
                }
                .into());
            }
        }
    }
    Ok(())
}

/// The provider and the module of each key `modules` declare. First listing
/// wins: it only names a module to import, and two imported modules
/// registering one key already failed the boot (`DuplicateProviderError`).
fn owners<'a>(
    modules: impl Iterator<Item = &'a ModuleDescriptor>,
) -> HashMap<TypeId, (&'static str, &'static str)> {
    let mut owners = HashMap::new();
    for d in modules {
        for p in d.providers {
            owners.entry((p.provides)()).or_insert((p.name, d.name));
            for (key, label) in (p.also_provides)() {
                owners.entry(key).or_insert((label, d.name));
            }
        }
    }
    owners
}

/// `ids` beside their labels, in order, a missing label read as unnamed.
pub(crate) fn labelled<'a>(
    ids: &'a [TypeId],
    names: &'a [&'static str],
) -> impl Iterator<Item = (TypeId, &'static str)> + 'a {
    ids.iter().enumerate().map(|(i, id)| {
        let name = names.get(i).copied().unwrap_or(UNNAMED_DEPENDENCY);
        (*id, name)
    })
}

/// Validate the **keyed** access graph: every reachable provider's
/// `#[inject(key = "…")]` dependency must be in the global keyed set (seeds +
/// factory outputs), since keyed providers have no per-module declaration.
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
                    key: dep.key.name.unwrap_or("<unnamed>"),
                });
            }
        }
    }
    Ok(())
}

/// The first cycle among the request-scoped and transient providers the roots
/// reach — each in `scopes` — in the order each waits on the next, through
/// what it injects, optionally or not: both resolve through the request's
/// scope when it is built. Walked depth-first in declaration order, so the
/// same app names the same chain. A factory registered by hand has no
/// descriptor, and its cycle is left to the panic at first resolution.
pub(crate) fn scoped_cycle(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    scopes: &HashMap<TypeId, ProviderScope>,
    overridden: &HashSet<TypeId>,
) -> Option<ProviderCycleError> {
    let graph = ProviderGraph::new(descriptors, roots, overridden, |p| {
        scopes.contains_key(&(p.provides)())
    });
    let cycle = graph.first_cycle(0..graph.nodes.len())?;
    let scope = scopes[&(graph.nodes[cycle[0]].1.provides)()];
    Some(graph.cycle_error(&cycle, scope))
}

/// The first cycle among the singletons the roots reach that one of `waiting`
/// — a provider by a key it registers — waits on, at any depth, through what
/// each injects, optionally or not. Singletons of one module are ordered, and
/// a cycle among them refused, by the register phase; across modules importing
/// each other, one module registers before the other's providers exist, so
/// the cycle shows only as a provider never built or built without an
/// optional dependency, from which the walk starts.
pub(crate) fn singleton_cycle(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    scopes: &HashMap<TypeId, ProviderScope>,
    waiting: &[TypeId],
    overridden: &HashSet<TypeId>,
) -> Option<ProviderCycleError> {
    let graph = ProviderGraph::new(descriptors, roots, overridden, |p| {
        !scopes.contains_key(&(p.provides)())
    });
    let starts = waiting
        .iter()
        .filter_map(|key| graph.node_of.get(key).copied());
    let cycle = graph.first_cycle(starts)?;
    Some(graph.cycle_error(&cycle, ProviderScope::Singleton))
}

/// The providers the roots reach that a filter keeps, one node per provider
/// type with its module, in declaration order, and an edge from each to the
/// node registering what it injects, optionally or not — none from a provider
/// a test's override stands in for, which is never built.
struct ProviderGraph<'a> {
    nodes: Vec<(&'a ModuleDescriptor, &'a ProviderDescriptor)>,
    node_of: HashMap<TypeId, usize>,
    edges: Vec<Vec<usize>>,
}

impl<'a> ProviderGraph<'a> {
    fn new(
        descriptors: &[&'a ModuleDescriptor],
        roots: &[TypeId],
        overridden: &HashSet<TypeId>,
        keep: impl Fn(&ProviderDescriptor) -> bool,
    ) -> Self {
        let by_id: HashMap<TypeId, &ModuleDescriptor> =
            descriptors.iter().map(|d| ((d.module)(), *d)).collect();
        let mut nodes: Vec<(&ModuleDescriptor, &ProviderDescriptor)> = Vec::new();
        let mut node_of = HashMap::new();
        let mut node_of_provider: HashMap<TypeId, usize> = HashMap::new();
        for module in reachable_in_order(roots, &by_id)
            .iter()
            .filter_map(|id| by_id.get(id))
        {
            for p in module.providers.iter().filter(|p| keep(p)) {
                // `Foo as dyn A, Foo as dyn B` is two descriptors of one provider.
                let node = *node_of_provider.entry((p.provider)()).or_insert_with(|| {
                    nodes.push((*module, p));
                    nodes.len() - 1
                });
                node_of.extend(provided_keys(p).map(|key| (key, node)));
            }
        }
        let edges = nodes
            .iter()
            .map(|(_, p)| {
                if overridden.contains(&(p.provider)()) {
                    return Vec::new();
                }
                (p.injects)()
                    .into_iter()
                    .chain((p.injects_optional)())
                    .filter_map(|key| node_of.get(&key).copied())
                    .collect()
            })
            .collect();
        Self {
            nodes,
            node_of,
            edges,
        }
    }

    /// The first cycle a walk from each of `starts`, in turn, closes.
    fn first_cycle(&self, mut starts: impl Iterator<Item = usize>) -> Option<Vec<usize>> {
        let mut done = vec![false; self.nodes.len()];
        starts.find_map(|start| cycle_from(start, &self.edges, &mut done))
    }

    /// `cycle` refused, named by its first provider's module.
    fn cycle_error(&self, cycle: &[usize], scope: ProviderScope) -> ProviderCycleError {
        ProviderCycleError {
            module: self.nodes[cycle[0]].0.name,
            scope,
            chain: cycle
                .iter()
                .map(|i| provider_name(self.nodes[*i].1))
                .collect(),
        }
    }
}

/// A provider by its own type, as its module lists it — `Foo` for a `Foo as
/// dyn Trait` binding, whose descriptor is named for the trait.
fn provider_name(provider: &ProviderDescriptor) -> &'static str {
    let own = (provider.provider)();
    if (provider.provides)() == own {
        return provider.name;
    }
    (provider.also_provides)()
        .into_iter()
        .find(|(key, _)| *key == own)
        .map_or(provider.name, |(_, label)| label)
}

/// The first cycle a depth-first walk from `start` closes, as the nodes from
/// the one it returns to, that one repeated last; `done` marks the nodes no
/// cycle passes through, across calls.
pub(crate) fn cycle_from(
    start: usize,
    edges: &[Vec<usize>],
    done: &mut [bool],
) -> Option<Vec<usize>> {
    if done[start] {
        return None;
    }
    let mut path = vec![start];
    let mut next_edge = vec![0usize];
    while let Some(&at) = path.last() {
        let edge = next_edge.last_mut()?;
        let Some(&to) = edges[at].get(*edge) else {
            done[at] = true;
            path.pop();
            next_edge.pop();
            continue;
        };
        *edge += 1;
        if let Some(on_path) = path.iter().position(|seen| *seen == to) {
            let mut cycle = path[on_path..].to_vec();
            cycle.push(to);
            return Some(cycle);
        }
        if !done[to] {
            path.push(to);
            next_edge.push(0);
        }
    }
    None
}

/// Why `missing`, absent when the module `stalled` built its providers, was
/// absent then though the same boot registered it: a module providing one of
/// them reaches `stalled` through its imports — an import cycle, which
/// `#[module]` breaks by registering one module before the other's providers
/// exist — or, failing that, a registration made later in the boot.
pub(crate) fn unbuilt_cause(
    reached: &[&ModuleDescriptor],
    stalled: TypeId,
    missing: &[TypeId],
) -> UnbuiltCause {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        reached.iter().map(|d| ((d.module)(), *d)).collect();
    for key in missing {
        let owners = reached.iter().filter(|d| {
            d.providers
                .iter()
                .flat_map(provided_keys)
                .any(|k| k == *key)
        });
        for owner in owners {
            let owner_id = (owner.module)();
            if owner_id == stalled {
                continue;
            }
            let (Some(there), Some(back)) = (
                import_path(stalled, owner_id, &by_id),
                import_path(owner_id, stalled, &by_id),
            ) else {
                continue;
            };
            // Both paths are shortest, yet may cross one module twice.
            let mut seen = HashSet::new();
            let modules = there
                .iter()
                .chain(&back[1..back.len() - 1])
                .filter(|id| seen.insert(**id))
                .filter_map(|id| by_id.get(id).map(|d| d.name))
                .collect();
            return UnbuiltCause::ImportCycle(modules);
        }
    }
    UnbuiltCause::RegisteredLater
}

/// Every key `provider` registers: its own, then each it registers on its
/// module's behalf.
fn provided_keys(provider: &ProviderDescriptor) -> impl Iterator<Item = TypeId> {
    std::iter::once((provider.provides)())
        .chain((provider.also_provides)().into_iter().map(|(key, _)| key))
}

/// The shortest import path from `from` to `to`, both included, along
/// `imports = [...]`.
fn import_path(
    from: TypeId,
    to: TypeId,
    by_id: &HashMap<TypeId, &ModuleDescriptor>,
) -> Option<Vec<TypeId>> {
    let mut came_from: HashMap<TypeId, TypeId> = HashMap::new();
    let mut queue = std::collections::VecDeque::from([from]);
    while let Some(id) = queue.pop_front() {
        if id == to {
            let mut path = vec![to];
            let mut at = to;
            while let Some(previous) = came_from.get(&at) {
                path.push(*previous);
                at = *previous;
            }
            path.reverse();
            return Some(path);
        }
        let Some(desc) = by_id.get(&id) else {
            continue;
        };
        for import in desc.imports.iter().map(|import| import()) {
            if import != from && !came_from.contains_key(&import) {
                came_from.insert(import, id);
                queue.push_back(import);
            }
        }
    }
    None
}

/// BFS over `imports` from `roots`, returning every module `TypeId` reached
/// (roots included). A `TypeId` without a descriptor terminates its branch.
fn reachable(roots: &[TypeId], by_id: &HashMap<TypeId, &ModuleDescriptor>) -> HashSet<TypeId> {
    reachable_in_order(roots, by_id).into_iter().collect()
}

/// The same walk, **in source order**: roots first, then each module's
/// `imports = [...]` left to right, depth-first, first occurrence winning.
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
            stack.extend(desc.imports.iter().rev().map(|import| import()));
        }
    }
    order
}

/// Provider keys the roots reach — `order`, from [`provider_order`] — plus
/// `global`, seeded as [`ReachableProviders`].
pub(crate) fn reachable_provider_ids(
    order: &[TypeId],
    global: &HashSet<TypeId>,
) -> HashSet<TypeId> {
    let mut keys = global.clone();
    keys.extend(order.iter().copied());
    keys
}

/// Every reachable provider in **declaration order**: modules depth-first from
/// the roots along their `imports = [...]`, and within each module its
/// `providers = [...]` left to right; first occurrence wins. A `Foo as dyn
/// Trait` binding is ranked under both keys it holds, the trait object first.
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
            for key in [(p.provides)(), (p.provider)()] {
                if seen.insert(key) {
                    order.push(key);
                }
            }
        }
    }
    order
}

/// Every reachable provider by its own type, **dependencies first**: each
/// after every provider it injects, optionally or not, at any depth. Providers
/// the graph does not order follow the composition — imported modules before
/// their importer, imports as declared, then each module's `providers = [...]`
/// left to right — and the earliest of them always goes next. A provider in
/// `overridden` is a test's double, which injects nothing.
pub(crate) fn dependency_order(
    descriptors: &[&ModuleDescriptor],
    roots: &[TypeId],
    overridden: &HashSet<TypeId>,
) -> Vec<TypeId> {
    let graph = ProviderGraph::new(descriptors, roots, overridden, |_| true);
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();
    let module_rank = positions(modules_imports_first(roots, &by_id));
    let tie: Vec<(usize, usize)> = graph
        .nodes
        .iter()
        .map(|(module, p)| {
            let at = module_rank.get(&(module.module)()).copied();
            let listed = module.providers.iter().position(|q| std::ptr::eq(q, *p));
            (at.unwrap_or(usize::MAX), listed.unwrap_or_default())
        })
        .collect();

    let nodes = graph.nodes.len();
    let mut waits_on = vec![0usize; nodes];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); nodes];
    for (node, mut deps) in graph.edges.into_iter().enumerate() {
        deps.sort_unstable();
        deps.dedup();
        deps.retain(|dep| *dep != node);
        waits_on[node] = deps.len();
        for dep in deps {
            dependents[dep].push(node);
        }
    }
    let mut ready: BinaryHeap<Reverse<((usize, usize), usize)>> = (0..nodes)
        .filter(|node| waits_on[*node] == 0)
        .map(|node| Reverse((tie[node], node)))
        .collect();
    let mut placed = vec![false; nodes];
    let mut order = Vec::with_capacity(nodes);
    while order.len() < nodes {
        // Only a cycle the seal already refused leaves nothing ready.
        let node = match ready.pop() {
            Some(Reverse((_, node))) => node,
            None => (0..nodes)
                .filter(|node| !placed[*node])
                .min_by_key(|node| tie[*node])
                .unwrap_or_default(),
        };
        placed[node] = true;
        order.push((graph.nodes[node].1.provider)());
        for dependent in &dependents[node] {
            waits_on[*dependent] = waits_on[*dependent].saturating_sub(1);
            if waits_on[*dependent] == 0 && !placed[*dependent] {
                ready.push(Reverse((tie[*dependent], *dependent)));
            }
        }
    }
    order
}

/// The modules `roots` reach, each after every module it imports: imports in
/// their declared order, depth-first, first occurrence winning.
fn modules_imports_first(
    roots: &[TypeId],
    by_id: &HashMap<TypeId, &ModuleDescriptor>,
) -> Vec<TypeId> {
    fn visit(
        id: TypeId,
        by_id: &HashMap<TypeId, &ModuleDescriptor>,
        seen: &mut HashSet<TypeId>,
        order: &mut Vec<TypeId>,
    ) {
        if !seen.insert(id) {
            return;
        }
        if let Some(desc) = by_id.get(&id) {
            for import in desc.imports {
                visit(import(), by_id, seen, order);
            }
        }
        order.push(id);
    }
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    for root in roots {
        visit(*root, by_id, &mut seen, &mut order);
    }
    order
}

/// Every key `root`'s code reaches through `#[inject]`, optional or not, at
/// any depth, `root` included: each provider `descriptors` declare under a key
/// in the set adds what it injects. A key no module declares — a seed, a
/// factory output — ends its branch. The caller passes the modules its app
/// reaches, so another composition's binding is never followed.
pub(crate) fn injection_closure(
    descriptors: &[&ModuleDescriptor],
    root: TypeId,
) -> HashSet<TypeId> {
    let mut reached = HashSet::from([root]);
    let mut stack = vec![root];
    while let Some(key) = stack.pop() {
        let declared = descriptors.iter().flat_map(|d| d.providers);
        for provider in declared.filter(|p| (p.provides)() == key || (p.provider)() == key) {
            for dependency in (provider.injects)()
                .into_iter()
                .chain((provider.injects_optional)())
            {
                if reached.insert(dependency) {
                    stack.push(dependency);
                }
            }
        }
    }
    reached
}

/// The descriptors of the modules `roots` reach along their imports, in no
/// particular order.
pub(crate) fn reachable_descriptors<'a>(
    descriptors: &[&'a ModuleDescriptor],
    roots: &[TypeId],
) -> Vec<&'a ModuleDescriptor> {
    let by_id: HashMap<TypeId, &ModuleDescriptor> =
        descriptors.iter().map(|d| ((d.module)(), *d)).collect();
    let reached = reachable(roots, &by_id);
    descriptors
        .iter()
        .copied()
        .filter(|d| reached.contains(&(d.module)()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::__private::ProviderDescriptor;
    use super::*;
    use crate::container::KeyedDependency;

    // Descriptors are hand-built so the global `inventory` registry is untouched.
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

    fn no_per_request() -> Vec<(TypeId, &'static str)> {
        Vec::new()
    }

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
                provider: || TypeId::of::<UsersService>(),
                injects: users_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
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
                    provider: || TypeId::of::<UsersService>(),
                    injects: no_deps,
                    inject_names: no_names,
                    injects_optional: no_deps,
                    inject_optional_names: no_names,
                    injects_keyed: no_keyed_deps,
                    injects_per_request: no_per_request,
                    also_provides: provides_only_itself,
                },
                ProviderDescriptor {
                    name: "AppGuard",
                    provides: || TypeId::of::<AppGuard>(),
                    provider: || TypeId::of::<AppGuard>(),
                    injects: billing_deps,
                    inject_names: no_names,
                    injects_optional: no_deps,
                    inject_optional_names: no_names,
                    injects_keyed: no_keyed_deps,
                    injects_per_request: no_per_request,
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
                provider: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
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
                provider: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
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
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                provider: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                inject_names: billing_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
                also_provides: provides_only_itself,
            }],
        };
        let err = validate_access_graph(
            &[&billing],
            &[TypeId::of::<BillingMod>()],
            &HashSet::new(),
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
                provider: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
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
                provider: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
                also_provides: provides_only_itself,
            }],
        };
        let keys = reachable_provider_ids(
            &provider_order(&[&app], &[TypeId::of::<AppMod>()]),
            &HashSet::new(),
        );
        assert!(keys.contains(&TypeId::of::<OrgsResolver>()));
    }

    #[test]
    fn a_provider_bound_as_dyn_is_reachable_and_ranked_under_its_own_type() {
        trait Port {}
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "dyn Port",
                provides: || TypeId::of::<std::sync::Arc<dyn Port>>(),
                provider: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
                also_provides: provides_only_itself,
            }],
        };
        let order = provider_order(&[&app], &[TypeId::of::<AppMod>()]);
        let rank = ProviderOrder::new(order.iter().copied());
        assert!(rank.rank(TypeId::of::<OrgsResolver>()) < usize::MAX);
        let keys = reachable_provider_ids(&order, &HashSet::new());
        assert!(keys.contains(&TypeId::of::<OrgsResolver>()));
        assert!(keys.contains(&TypeId::of::<std::sync::Arc<dyn Port>>()));
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
                provider: || TypeId::of::<OrgsResolver>(),
                injects: no_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
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
            &provider_order(&[&app, &billing], &[TypeId::of::<AppMod>()]),
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
        let keys = reachable_provider_ids(
            &provider_order(&[&app], &[TypeId::of::<AppMod>()]),
            &global(),
        );
        assert!(keys.contains(&TypeId::of::<Db>()));
    }

    // One concrete type injected twice, disambiguated by key.
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
                provider: || TypeId::of::<UsersService>(),
                injects: no_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: github_dep,
                injects_per_request: no_per_request,
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

    #[test]
    fn the_injection_closure_follows_every_declared_injection_and_stops_at_what_none_declares() {
        let users = users_module();
        let billing = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BillingModule",
            imports: &[|| TypeId::of::<UsersMod>()],
            providers: &[ProviderDescriptor {
                name: "BillingService",
                provides: || TypeId::of::<BillingService>(),
                provider: || TypeId::of::<BillingService>(),
                injects: billing_deps,
                inject_names: billing_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
                also_provides: provides_only_itself,
            }],
        };
        let descriptors = [&billing, &users];
        assert_eq!(
            injection_closure(&descriptors, TypeId::of::<BillingService>()),
            HashSet::from([
                TypeId::of::<BillingService>(),
                TypeId::of::<UsersService>(),
                TypeId::of::<Db>(),
            ])
        );
        assert_eq!(
            injection_closure(&descriptors, TypeId::of::<OrgsResolver>()),
            HashSet::from([TypeId::of::<OrgsResolver>()])
        );
    }

    // Named against the alphabet and declared against the dependencies.
    struct Zdb;
    struct Mrepo;
    struct Aservice;

    fn injects_zdb() -> Vec<TypeId> {
        vec![TypeId::of::<Zdb>()]
    }

    fn injects_mrepo() -> Vec<TypeId> {
        vec![TypeId::of::<Mrepo>()]
    }

    #[test]
    fn a_provider_ranks_after_everything_it_injects_at_any_depth() {
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[
                ProviderDescriptor {
                    name: "Aservice",
                    provides: || TypeId::of::<Aservice>(),
                    provider: || TypeId::of::<Aservice>(),
                    injects: injects_mrepo,
                    inject_names: no_names,
                    injects_optional: no_deps,
                    inject_optional_names: no_names,
                    injects_keyed: no_keyed_deps,
                    injects_per_request: no_per_request,
                    also_provides: provides_only_itself,
                },
                ProviderDescriptor {
                    name: "Mrepo",
                    provides: || TypeId::of::<Mrepo>(),
                    provider: || TypeId::of::<Mrepo>(),
                    injects: no_deps,
                    inject_names: no_names,
                    injects_optional: injects_zdb,
                    inject_optional_names: no_names,
                    injects_keyed: no_keyed_deps,
                    injects_per_request: no_per_request,
                    also_provides: provides_only_itself,
                },
                ProviderDescriptor {
                    name: "Zdb",
                    provides: || TypeId::of::<Zdb>(),
                    provider: || TypeId::of::<Zdb>(),
                    injects: no_deps,
                    inject_names: no_names,
                    injects_optional: no_deps,
                    inject_optional_names: no_names,
                    injects_keyed: no_keyed_deps,
                    injects_per_request: no_per_request,
                    also_provides: provides_only_itself,
                },
            ],
        };
        assert_eq!(
            dependency_order(&[&app], &[TypeId::of::<AppMod>()], &HashSet::new()),
            [
                TypeId::of::<Zdb>(),
                TypeId::of::<Mrepo>(),
                TypeId::of::<Aservice>()
            ],
        );
    }

    trait Port {}

    fn injects_port() -> Vec<TypeId> {
        vec![TypeId::of::<std::sync::Arc<dyn Port>>()]
    }

    fn also_its_own_type() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Zdb>(), "Zdb")]
    }

    #[test]
    fn a_provider_bound_as_dyn_ranks_under_its_own_type_before_what_injects_the_trait() {
        let app = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AppModule",
            imports: &[],
            providers: &[
                ProviderDescriptor {
                    name: "Aservice",
                    provides: || TypeId::of::<Aservice>(),
                    provider: || TypeId::of::<Aservice>(),
                    injects: injects_port,
                    inject_names: no_names,
                    injects_optional: no_deps,
                    inject_optional_names: no_names,
                    injects_keyed: no_keyed_deps,
                    injects_per_request: no_per_request,
                    also_provides: provides_only_itself,
                },
                ProviderDescriptor {
                    name: "dyn Port",
                    provides: || TypeId::of::<std::sync::Arc<dyn Port>>(),
                    provider: || TypeId::of::<Zdb>(),
                    injects: no_deps,
                    inject_names: no_names,
                    injects_optional: no_deps,
                    inject_optional_names: no_names,
                    injects_keyed: no_keyed_deps,
                    injects_per_request: no_per_request,
                    also_provides: also_its_own_type,
                },
            ],
        };
        assert_eq!(
            dependency_order(&[&app], &[TypeId::of::<AppMod>()], &HashSet::new()),
            [TypeId::of::<Zdb>(), TypeId::of::<Aservice>()],
        );
    }

    struct CMod;

    #[test]
    fn unrelated_providers_rank_imports_first_as_declared_then_the_importer() {
        let b = ModuleDescriptor {
            module: || TypeId::of::<BillingMod>(),
            name: "BModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "Mrepo",
                provides: || TypeId::of::<Mrepo>(),
                provider: || TypeId::of::<Mrepo>(),
                injects: no_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
                also_provides: provides_only_itself,
            }],
        };
        let c = ModuleDescriptor {
            module: || TypeId::of::<CMod>(),
            name: "CModule",
            imports: &[],
            providers: &[ProviderDescriptor {
                name: "Zdb",
                provides: || TypeId::of::<Zdb>(),
                provider: || TypeId::of::<Zdb>(),
                injects: no_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
                also_provides: provides_only_itself,
            }],
        };
        let a = ModuleDescriptor {
            module: || TypeId::of::<AppMod>(),
            name: "AModule",
            imports: &[|| TypeId::of::<BillingMod>(), || TypeId::of::<CMod>()],
            providers: &[ProviderDescriptor {
                name: "Aservice",
                provides: || TypeId::of::<Aservice>(),
                provider: || TypeId::of::<Aservice>(),
                injects: no_deps,
                inject_names: no_names,
                injects_optional: no_deps,
                inject_optional_names: no_names,
                injects_keyed: no_keyed_deps,
                injects_per_request: no_per_request,
                also_provides: provides_only_itself,
            }],
        };
        assert_eq!(
            dependency_order(&[&a, &c, &b], &[TypeId::of::<AppMod>()], &HashSet::new()),
            [
                TypeId::of::<Mrepo>(),
                TypeId::of::<Zdb>(),
                TypeId::of::<Aservice>()
            ],
        );
    }
}
