//! The [`Discoverable`] trait — the contract a `#[module]` uses to register a
//! provider and report its dependencies to the access-graph check.

use std::any::TypeId;
use std::borrow::Cow;

use crate::access::{Composition, ModuleDescriptor};
use crate::container::{Container, ContainerBuilder, KeyedDependency};

/// Anything a `#[module]` can pull in via `providers = [...]`.
///
/// Decorator macros (`#[injectable]`, `#[interceptor]`, `#[scheduled]`,
/// `#[mcp]`, `#[routes]`, …) emit a single `impl Discoverable for Self` that
/// either registers a provider or attaches discovery metadata.
pub trait Discoverable {
    /// Provider types that must already be registered before
    /// [`register`](Discoverable::register) can build this one — read by
    /// `#[module]` to order registration. Empty for providers built lazily
    /// (controllers, resolvers) so they do not block the register-phase
    /// fixpoint.
    fn dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject]` dependency, recorded for the access-graph
    /// check. Reported regardless of build timing, so the contract governs
    /// transport-built logic too.
    fn injected() -> Vec<TypeId> {
        Vec::new()
    }

    /// Human-readable label for each [`injected`](Discoverable::injected)
    /// entry, in the same order, so the access graph can name a dependency no
    /// module provides — a lazily-built provider's missing dependency is a clean
    /// boot error naming both the provider and the dependency, not a
    /// `get(...).expect(...)` panic at first resolution. May be shorter than
    /// `injected()` (a provider that does not emit names falls back to a
    /// placeholder); never longer.
    fn injected_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// [`ProviderKey`](crate::ProviderKey) of each **keyed** `#[inject(key = "…")]` dependency,
    /// recorded for the access-graph keyed check. Kept apart from
    /// [`injected`](Discoverable::injected) — a keyed dependency is validated
    /// against the global keyed set (seeds + factory outputs), and its boot
    /// error names both the type and the key. Empty for providers with no keyed
    /// dependency (the default).
    fn injected_keyed() -> Vec<KeyedDependency> {
        Vec::new()
    }

    /// Human-readable label for each [`dependencies`](Discoverable::dependencies)
    /// entry, in the same order, so the boot-time fixpoint can name a missing
    /// dependency.
    fn dependency_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject] Option<Arc<…>>` optional dependency.
    /// Not required by the register-phase fixpoint, but
    /// used to order the provider after an optional dependency the same module
    /// supplies.
    fn optional_dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// Container keys this provider registers **besides itself**, each with the
    /// label a boot error should use for it.
    ///
    /// A provider normally registers exactly one key — its own type — and
    /// `#[module]` records that automatically. This hook is for the provider that
    /// also installs a *typed singleton on its module's behalf*, so the access
    /// graph can attribute that key to the module and produce the same named
    /// error it gives for any other unimported dependency. `nest-rs-ws` uses it
    /// for the per-namespace `WsServer<N>` registries `WsModule` owns: without
    /// it, the key belongs to no module, and the graph's escape hatch for
    /// imperatively-registered types waves the dependency through — then the
    /// consumer panics at first resolution, naming the wrong provider.
    ///
    /// Empty for every ordinary provider.
    fn also_provides() -> Vec<(TypeId, &'static str)> {
        Vec::new()
    }

    /// Install this provider's construction into the builder — the register
    /// phase's per-provider step. Emitted by the decorator (`#[injectable]`,
    /// `#[routes]`, …); resolves the provider's dependencies from the builder
    /// and stores the built value plus any metadata.
    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}

/// What the container holds under a provider's **own type**, stated by whatever
/// decorator built it — and read by the impl halves that resolve their host
/// there (`#[hooks]`, `#[scheduled]`, `#[listeners]`, `#[indicators]`,
/// `#[processor]`).
///
/// Those five resolve with `Container::get::<Host>()` at boot, at a tick, at a
/// published event, at a probe, at a job — always outside any request. That
/// call answers correctly for exactly one registration shape, **a singleton
/// stored under its own type**, and each of the others fails in its own quiet
/// way:
///
/// | Host | What `get` does | Symptom if it were allowed |
/// |---|---|---|
/// | edge host (`#[controller]`, `#[gateway]`, `#[resolver]`, `#[mcp]`) | `None` — the type registers *metadata*, an instance is built at mount | the edge serves, the method never runs, one `warn` |
/// | `#[injectable(scope = request)]` | `None` — the container holds a factory, not a value | the same `warn`, misnaming the cause |
/// | `#[injectable(scope = transient)]` | builds a **throwaway** instance | the method runs, its effects are dropped, and nothing warns at all |
///
/// The fact is **stated, never omitted**: every decorator that builds a provider
/// writes this impl, `true` or `false`. That is what makes contradicting it a
/// coherence error rather than a second opinion — a marker merely *absent* for
/// the shapes it refuses can be filled in by hand, and was. The escape hatch
/// survives only where nothing has spoken, on a provider registered by hand with
/// [`ContainerBuilder::provide`]:
///
/// ```ignore
/// impl ProviderResidency for MyHandWrittenProvider {
///     const SINGLETON: bool = true;
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a provider, so a provider-hosted decorator (`#[hooks]`, \
               `#[scheduled]`, `#[listeners]`, `#[indicators]`, `#[processor]`) cannot reach it",
    label = "the container holds no singleton of this type",
    note = "those decorators resolve their host with `Container::get::<Self>()`, outside any \
            request, so the host must be a provider the container holds under its own type. Add \
            `#[injectable]` — or, on a provider registered by hand with `ContainerBuilder::provide`, \
            write `impl ProviderResidency` for it with `const SINGLETON: bool = true`"
)]
pub trait ProviderResidency {
    /// `true` when the container holds **one instance of this provider, under
    /// this very type, for the application's lifetime** — `false` for every
    /// other shape in the table above.
    const SINGLETON: bool;
}

/// The sentence a discovery site appends when it skips an operation whose host
/// the booted container does not hold, and the boot cannot tell why — the
/// run-time half ([`unresolved_host`]), or a container built without the
/// composition [`inert_host`] reads.
///
/// Its wording is the third attempt, and the first two are why it **names
/// causes and prescribes nothing**:
///
/// * *"provider unreachable from app's module tree"* was false whenever the
///   provider was written right there in `providers` — bound as `dyn Trait`,
///   imported through a `for_root`, or registered by a hand-written `Module`.
/// * Naming the dyn case and offering *"list it under its own type as well"*
///   was worse: `providers = [Foo, Foo as dyn Trait]` runs the provider's
///   constructor **twice**. The decorators then fire on one instance while every
///   consumer injecting `Arc<dyn Trait>` holds the other, and nothing warns —
///   the exact silent shape [`ProviderResidency`] exists to refuse, reached by
///   *following* the advice. On a hand-written `impl Module` the same edit
///   fails the boot outright with `DuplicateProviderError`.
///
/// A remedy is only safe to print where the framework knows which cause it is
/// looking at. At boot it now does — [`InertHost`] names the cause and its
/// remedy — so this sentence is left to the two sites that cannot.
pub const INERT_HOST_HINT: &str = concat!(
    "nothing is registered under that exact type. Common causes: its module is not imported ",
    "by this app (import it, or delete the methods); it is bound only as `dyn Trait`; it is ",
    "imported through a `for_root`; it is registered by hand under a key or a trait",
);

/// Why a discovered host is inert in this app — which decides whether the boot
/// says so at all, and what it tells the developer to do. Read by
/// [`report_inert_host!`], from [`inert_host`].
///
/// **A host a library crate holds and this app does not import is another
/// app's**, and is said at `debug`. In a workspace of several binaries one
/// library crate holds the hosts every binary imports some of, so each binary
/// links the others': the demo's worker reported three of the api's scheduled
/// methods and one of its shutdown hooks at `warn` on every boot, with a hint
/// telling it to import them. The configuration report settled the same shape
/// the same way — a namespace this binary does not link is another binary's —
/// and the binary's own crate keeps its `warn`, where leftover code is the
/// developer's to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InertHost {
    /// A `nest-rs-*` capability the app never opted into — see
    /// [`is_framework_owned`]. Said at `debug`.
    Framework,
    /// A library crate's host this app neither imports the module of nor
    /// registers: another binary's, or nobody's. Said at `debug`.
    AnotherApps {
        /// A module listing it, which this app does not import — `None` when no
        /// `#[module]` lists it, so only a hand-written registration in another
        /// binary could run it.
        module: Option<&'static str>,
    },
    /// The app's own host, declared by a module this app does not import.
    ModuleNotImported {
        /// That module.
        module: &'static str,
    },
    /// The app's own host, listed in no `#[module]`'s `providers` and held by
    /// nothing under its own type: registered by nothing, or by hand or through
    /// a `for_root` under a trait — which the boot cannot tell apart, and whose
    /// remedies differ.
    NotListed,
    /// Held under its own type by this app's container, and listed by no module
    /// it imports: registered by hand or through a `for_root`, outside the
    /// `providers` lists discovery reads. A host the app registered is the
    /// app's, whatever crate it is written in.
    RegisteredOutsideModules,
    /// Listed by a module this app imports under another key only — `Foo as
    /// dyn Trait` — while a decorated method resolves its host under its own
    /// type.
    BoundUnderAnotherKey {
        /// The module that lists it.
        module: &'static str,
        /// The key it is listed under, as the module's descriptor names it.
        key: &'static str,
    },
    /// Listed under its own type by a module this app imports, and absent all
    /// the same — or the container carries no composition to read — so the
    /// cause is not known.
    Unknown,
}

impl InertHost {
    /// Whether the developer of this app can act on it — said at `warn` when
    /// so, at `debug` otherwise.
    pub fn is_the_apps(self) -> bool {
        !matches!(self, Self::Framework | Self::AnotherApps { .. })
    }

    /// A low-cardinality name for the cause, the `cause` field of the line.
    pub fn cause(self) -> &'static str {
        match self {
            Self::Framework => "framework",
            Self::AnotherApps { .. } => "another_app",
            Self::ModuleNotImported { .. } => "module_not_imported",
            Self::NotListed => "not_listed",
            Self::RegisteredOutsideModules => "registered_outside_modules",
            Self::BoundUnderAnotherKey { .. } => "bound_under_another_key",
            Self::Unknown => "unknown",
        }
    }

    /// What the developer does about it — the remedy the cause has, which the
    /// boot can name because it knows the cause; [`INERT_HOST_HINT`]'s list of
    /// causes when it does not.
    pub fn hint(self) -> Cow<'static, str> {
        match self {
            Self::Framework => Cow::Borrowed("a capability of the framework this app does not use"),
            Self::AnotherApps {
                module: Some(module),
            } => Cow::Owned(format!(
                "a library crate's host whose module `{module}` this app does not import: another \
                 binary's to run"
            )),
            Self::AnotherApps { module: None } => Cow::Borrowed(
                "a library crate's host that no `#[module]` lists and this app does not register: \
                 another binary's to register by hand, or nobody's",
            ),
            Self::ModuleNotImported { module } => Cow::Owned(format!(
                "its module `{module}` is not imported by this app: import it, or delete the methods"
            )),
            Self::NotListed => Cow::Borrowed(
                "no `#[module]` lists it and nothing registers it under its own type: if nothing \
                 registers it at all, list it in a `#[module]`'s `providers` or delete the \
                 methods; if a hand-written module or a `for_root` binds it under a trait, move \
                 the methods to a provider registered under its own type",
            ),
            Self::RegisteredOutsideModules => Cow::Borrowed(
                "this app registers it by hand or through a `for_root`, and discovery reads only \
                 the `providers` of the `#[module]`s it imports: list it in one of those instead \
                 of registering it by hand",
            ),
            Self::BoundUnderAnotherKey { module, key } => Cow::Owned(format!(
                "`{module}` lists it only as `{key}`, and a decorated method resolves its host \
                 under its own type: move the methods to a provider listed under its own type — \
                 listing this one a second time, under its own type, would build it twice"
            )),
            Self::Unknown => Cow::Borrowed(INERT_HOST_HINT),
        }
    }
}

/// Why the host `host`, whose decorator recorded `origin` (its
/// `module_path!()`), is inert in the app `container` was booted for.
///
/// Read off the module descriptors every `#[module]` files, the
/// [`Composition`] the boot seeds and the container: which modules list the
/// host, under which key, whether the app reaches them, whether the app holds
/// it under its own type all the same, and whether the host is written in a
/// crate the app's roots are. Without a composition — a container built by
/// hand in a test — the cause is [`Unknown`](InertHost::Unknown), said at
/// `warn` as every inert host was before the composition existed.
pub fn inert_host(container: &Container, origin: &str, host: TypeId) -> InertHost {
    if is_framework_owned(origin) {
        return InertHost::Framework;
    }
    let Some(composition) = container.get::<Composition>() else {
        return InertHost::Unknown;
    };
    let descriptors: Vec<&ModuleDescriptor> =
        crate::inventory::iter::<ModuleDescriptor>().collect();
    classify(
        &descriptors,
        &composition,
        origin,
        host,
        container.holds(host),
    )
}

/// [`inert_host`] over `descriptors`, for a host the framework does not own,
/// which the app's container holds under its own type when `held`. Pure over
/// its inputs.
fn classify(
    descriptors: &[&ModuleDescriptor],
    composition: &Composition,
    origin: &str,
    host: TypeId,
    held: bool,
) -> InertHost {
    let mut unreached = None;
    for module in descriptors {
        for provider in module.providers.iter().filter(|p| (p.provider)() == host) {
            if !composition.reaches((module.module)()) {
                unreached.get_or_insert(module.name);
            } else if (provider.provides)() != host {
                return InertHost::BoundUnderAnotherKey {
                    module: module.name,
                    key: provider.name,
                };
            } else {
                return InertHost::Unknown;
            }
        }
    }
    if held {
        return InertHost::RegisteredOutsideModules;
    }
    if !composition.is_the_apps_own(origin) {
        return InertHost::AnotherApps { module: unreached };
    }
    match unreached {
        Some(module) => InertHost::ModuleNotImported { module },
        None => InertHost::NotListed,
    }
}

/// The error a discovery thunk reports when the booted container does not hold
/// its own host — the **run-time** half of the concern [`INERT_HOST_HINT`]
/// answers at boot.
///
/// Here rather than in any one capability's crate, because five decorators
/// resolve their host the same way (`#[hooks]`, `#[process]`, `#[indicators]`,
/// `#[listeners]`, `#[scheduled]`) and the fact they report is one fact. Written
/// at a capability, the second decorator to want it copies it — which is how two
/// of the five came to print *"add it to a reachable module's `providers =
/// [...]`"*, an imperative this module's own tests assert the shared sentence
/// must never carry: following it constructs the provider twice.
///
/// An error rather than a panic wherever the caller can carry one: these thunks
/// run inside a request or a tick, where a panic takes the response down while
/// an `Err` is an outcome the surface already renders.
///
/// A `*-macros` crate reaches this through its own surface crate's re-export
/// (`::nest_rs_health::unresolved_host`), never across to a sibling.
pub fn unresolved_host(host: &str) -> anyhow::Error {
    anyhow::Error::msg(format!(
        "host `{host}` could not be resolved: {INERT_HOST_HINT}"
    ))
}

/// Whether an inert operation belongs to the **framework** rather than to the
/// app, read from the `module_path!()` its decorator recorded.
///
/// A `nest-rs-*` crate is linked as soon as *any* of its capabilities is used,
/// so its opt-in providers — the ones behind a `for_root` the app did not
/// import — are inert in the normal case: `nest_rs_seaorm`'s `db` indicator in
/// an app that imports `SeaOrmDatabaseModule` without `SeaOrmHealthModule`,
/// `nest_rs_authn`'s audience check in every app that does not run a resource
/// server. The developer cannot act on those and they are not mistakes, so they
/// report at `debug`; the app's own inert code stays at `warn`, where the
/// module-gated discovery rule wants it.
///
/// Shared, because it belongs to every discovery site and lived at exactly one:
/// two demo apps warned twice per boot about a `nest-rs-seaorm` indicator —
/// telling the developer to go bind a framework-internal type — while the
/// lifecycle site got the same call right in the same boot.
///
/// **Known limit.** This tests the origin's *crate segment*, so it answers
/// `true` for any crate named `nest_rs_…`, including a third-party plugin that
/// takes the framework's prefix: such a crate's own inert operation is demoted
/// to `debug` and told it is a framework capability. The authoritative answer
/// exists at expansion time — `nest_rs_codegen`'s umbrella resolution already
/// separates "compiled inside the framework" from "compiled against it" — but
/// carrying it here means a new field on all five inventory structs. Recorded
/// as an owner question rather than guessed at more cleverly.
pub fn is_framework_owned(origin: &str) -> bool {
    let krate = origin.split("::").next().unwrap_or(origin);
    krate == "nest_rs" || krate.starts_with("nest_rs_")
}

/// Report a discovered operation whose host the booted container does not hold,
/// at the level its owner earns, with the remedy its cause has: [`inert_host`]
/// decides. `debug` for a `nest-rs-*` capability the app never opted into and
/// for another binary's host in a shared library crate; `warn` for the app's
/// own — leftover code, a module not imported, a provider bound under another
/// key — with a `cause` and a `hint` naming what to do.
///
/// A macro and not a function, and `tracing` decides that: `target:` and the
/// level land inside a `static` callsite initializer, so both must be const at
/// the call site. Its paths are `$crate::tracing::`, never `::tracing::` — an
/// exported macro's expansion lands in the *caller's* crate and resolves
/// against the caller's extern prelude, so a bare path is an `E0433` inside an
/// expansion nobody can read the day a consumer declares no `tracing`. The
/// kernel's other exported macro was fixed for that reason and wrote it down;
/// this one had kept the bare form. Folding the five targets into one would move four crates'
/// skip lines off the target the observability table assigns them.
///
/// The branch and its sentences lived five times, and the same refactor had
/// been performed at two sites and skipped at three — so they live here. A new
/// edge owes an inert-entry report, so a sixth site is scheduled rather than
/// hypothetical.
///
/// ```ignore
/// report_inert_host!(
///     target: crate::target::LIFECYCLE,
///     what: "scheduled method",
///     origin: entry.origin,
///     host: (entry.provider_type_id)(),
///     container: container,
///     provider = entry.provider,
///     method = entry.method,
/// );
/// ```
#[macro_export]
macro_rules! report_inert_host {
    (target: $target:expr, what: $what:literal, origin: $origin:expr, host: $host:expr, container: $container:expr $(, $field:ident = $value:expr)* $(,)?) => {{
        let __origin = $origin;
        let __inert = $crate::inert_host($container, __origin, $host);
        if __inert.is_the_apps() {
            $crate::tracing::warn!(
                target: $target,
                $($field = $value,)*
                origin = __origin,
                cause = __inert.cause(),
                hint = %__inert.hint(),
                ::core::concat!(
                    "skipped ", $what, ": no instance of the provider in this app's container",
                ),
            );
        } else {
            $crate::tracing::debug!(
                target: $target,
                $($field = $value,)*
                origin = __origin,
                cause = __inert.cause(),
                hint = %__inert.hint(),
                ::core::concat!("skipped ", $what, ": not this app's to run"),
            );
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hint must name the causes and **prescribe nothing**. Its second
    /// wording offered `providers = [Foo, Foo as dyn Trait]`, which silently
    /// constructs the provider twice — so this pins both halves: the causes are
    /// there, and no imperative that could be followed into that shape is.
    // Which *level* an inert hook is reported at, and why it matters: a `warn`
    // naming a framework-internal provider shows up on every freshly scaffolded
    // auth app (`AudienceBinding`, behind a `OAuthResourceModule` nobody
    // imported), is not actionable, and teaches the reader to ignore a target
    // that also carries security events.
    #[test]
    fn a_framework_owned_origin_is_not_the_developers_problem() {
        for origin in [
            "nest_rs_oauth_resource::module",
            "nest_rs_events::module",
            "nest_rs",
            "nest_rs::authn",
        ] {
            assert!(is_framework_owned(origin), "{origin}");
        }
        // An app's own provider still warns — that is leftover code the
        // developer can act on, and the whole reason the report exists.
        for origin in [
            "features::users::service",
            "api::module",
            "my_nest_rs_helpers::hooks",
        ] {
            assert!(!is_framework_owned(origin), "{origin}");
        }
    }

    /// The known limit, pinned so it is a decision rather than a surprise: a
    /// third-party crate that takes the framework's prefix is read as the
    /// framework's. Found by an audit whose own probe crate was called
    /// `nest-rs-audit-probe` and watched its app-owned hook get demoted.
    #[test]
    fn a_third_party_crate_taking_the_prefix_is_read_as_the_frameworks() {
        assert!(is_framework_owned("nest_rs_audit_probe"));
    }

    /// The hint must name the causes and **prescribe nothing**.
    ///
    /// The first version of this assertion was `!contains("as well")` — a
    /// tripwire on one historic wording rather than on the property. A mutation
    /// test put the same forbidden edit back in different words (*"Also add
    /// `providers = [Foo, Foo as dyn Trait]` …"*) and it passed. So the guard is
    /// on the **shape**: that remedy cannot be written without naming a
    /// `providers` list, and a prescription reads as an imperative.
    /// `listing_a_host_both_ways_builds_it_twice` is what proves the edit must
    /// never be printed.
    #[test]
    fn the_inert_host_hint_names_the_causes_and_prescribes_no_edit() {
        for cause in ["not imported", "dyn Trait", "for_root", "by hand"] {
            assert!(INERT_HOST_HINT.contains(cause), "{INERT_HOST_HINT}");
        }
        assert!(
            !INERT_HOST_HINT.contains("providers = ["),
            "naming a `providers` list is how the double-listing remedy is written, and it \
             constructs the provider twice: {INERT_HOST_HINT}",
        );
        for imperative in [
            "Also add", "also add", "List it", "list it", "as well", "Add `",
        ] {
            assert!(
                !INERT_HOST_HINT.contains(imperative),
                "the hint names causes; only a site that knows which one applies may prescribe \
                 an edit, and this one cannot: {INERT_HOST_HINT}",
            );
        }
    }

    mod classify {
        use std::any::TypeId;

        use super::super::{InertHost, classify};
        use crate::access::{Composition, ModuleDescriptor, ProviderDescriptor};
        use crate::container::KeyedDependency;

        struct AppModule;
        struct WorkerTasksModule;
        struct BoundModule;
        struct Tasks;
        struct Bound;
        struct Unlisted;
        trait Port {}

        fn nothing() -> Vec<TypeId> {
            Vec::new()
        }
        fn no_names() -> Vec<&'static str> {
            Vec::new()
        }
        fn no_keyed() -> Vec<KeyedDependency> {
            Vec::new()
        }
        fn only_itself() -> Vec<(TypeId, &'static str)> {
            Vec::new()
        }

        /// A workspace's composition: the app's root imports `BoundModule`,
        /// which lists `Bound` only as `dyn Port`; `WorkerTasksModule` lists
        /// `Tasks` and is imported by no root of this app.
        fn workspace() -> (Vec<ModuleDescriptor>, Composition) {
            let modules = vec![
                ModuleDescriptor {
                    module: || TypeId::of::<AppModule>(),
                    name: "AppModule",
                    imports: &[|| TypeId::of::<BoundModule>()],
                    providers: &[],
                },
                ModuleDescriptor {
                    module: || TypeId::of::<BoundModule>(),
                    name: "BoundModule",
                    imports: &[],
                    providers: &[ProviderDescriptor {
                        name: "dyn Port",
                        provides: || TypeId::of::<std::sync::Arc<dyn Port>>(),
                        provider: || TypeId::of::<Bound>(),
                        also_provides: only_itself,
                        injects: nothing,
                        inject_names: no_names,
                        injects_keyed: no_keyed,
                    }],
                },
                ModuleDescriptor {
                    module: || TypeId::of::<WorkerTasksModule>(),
                    name: "WorkerTasksModule",
                    imports: &[],
                    providers: &[ProviderDescriptor {
                        name: "Tasks",
                        provides: || TypeId::of::<Tasks>(),
                        provider: || TypeId::of::<Tasks>(),
                        also_provides: only_itself,
                        injects: nothing,
                        inject_names: no_names,
                        injects_keyed: no_keyed,
                    }],
                },
            ];
            let refs: Vec<&ModuleDescriptor> = modules.iter().collect();
            let composition = Composition::from_descriptors(
                &refs,
                &[(TypeId::of::<AppModule>(), "api::module::ApiModule")],
            );
            (modules, composition)
        }

        fn classified(origin: &str, host: TypeId) -> InertHost {
            held_or_not(origin, host, false)
        }

        fn held_or_not(origin: &str, host: TypeId, held: bool) -> InertHost {
            let (modules, composition) = workspace();
            let refs: Vec<&ModuleDescriptor> = modules.iter().collect();
            classify(&refs, &composition, origin, host, held)
        }

        /// Q1: a library crate's host whose module another binary imports was
        /// a `warn` telling this one to import it — the demo's worker filed four
        /// on every boot of a correct workspace. It is another app's, at
        /// `debug`.
        #[test]
        fn a_library_hosts_module_this_app_does_not_import_is_another_apps() {
            let inert = classified("features::tasks", TypeId::of::<Tasks>());
            assert_eq!(
                inert,
                InertHost::AnotherApps {
                    module: Some("WorkerTasksModule")
                }
            );
            assert!(!inert.is_the_apps(), "said at debug");
            assert!(
                inert.hint().contains("`WorkerTasksModule`"),
                "{}",
                inert.hint()
            );
        }

        /// The same host in the app's own crate is leftover code: a `warn`
        /// naming the module to import.
        #[test]
        fn the_apps_own_host_names_the_module_it_does_not_import() {
            let inert = classified("api::tasks", TypeId::of::<Tasks>());
            assert_eq!(
                inert,
                InertHost::ModuleNotImported {
                    module: "WorkerTasksModule"
                }
            );
            assert!(inert.is_the_apps());
            assert!(
                inert.hint().contains("`WorkerTasksModule` is not imported"),
                "{}",
                inert.hint()
            );
        }

        /// A host bound only as `dyn Trait` in a module the app imports is a
        /// mistake whatever crate it is in: the remedy names the binding.
        #[test]
        fn a_host_bound_under_another_key_names_the_binding() {
            for origin in ["api::bound", "features::bound"] {
                let inert = classified(origin, TypeId::of::<Bound>());
                assert_eq!(
                    inert,
                    InertHost::BoundUnderAnotherKey {
                        module: "BoundModule",
                        key: "dyn Port"
                    },
                    "{origin}"
                );
                assert!(inert.is_the_apps());
                assert!(
                    !inert.hint().contains("providers = ["),
                    "the remedy never prescribes the double listing that builds it twice: {}",
                    inert.hint()
                );
            }
        }

        /// A host no module lists and nothing holds under its own type: the
        /// app's own is a `warn`, a library's another app's — and neither hint
        /// claims a module that does not exist. The app's hint names both
        /// remedies, since a hand-written module binding it under a trait is a
        /// cause the boot cannot see, and listing it then builds it twice.
        #[test]
        fn a_host_no_module_lists_is_named_so_when_it_is_the_apps() {
            let own = classified("api::unlisted", TypeId::of::<Unlisted>());
            assert_eq!(own, InertHost::NotListed);
            assert!(own.is_the_apps());
            assert!(
                own.hint().contains("if nothing registers it at all")
                    && own.hint().contains("binds it under a trait"),
                "{}",
                own.hint()
            );
            let library = classified("features::unlisted", TypeId::of::<Unlisted>());
            assert_eq!(library, InertHost::AnotherApps { module: None });
            assert!(!library.is_the_apps());
            assert!(
                library.hint().contains("no `#[module]` lists"),
                "a hint naming a module nothing declares sent the reader to look for it: {}",
                library.hint()
            );
        }

        /// A host the app's container holds under its own type, outside every
        /// module it imports, was registered by this app — by hand or through a
        /// `for_root` — so it is the app's whichever crate it is written in, and
        /// the remedy is to list it where discovery reads, *instead of* the
        /// hand registration rather than beside it.
        #[test]
        fn a_host_the_app_registers_outside_its_modules_is_the_apps() {
            for (origin, host) in [
                ("features::unlisted", TypeId::of::<Unlisted>()),
                ("features::tasks", TypeId::of::<Tasks>()),
                ("api::unlisted", TypeId::of::<Unlisted>()),
            ] {
                let inert = held_or_not(origin, host, true);
                assert_eq!(inert, InertHost::RegisteredOutsideModules, "{origin}");
                assert!(inert.is_the_apps(), "{origin}");
                assert!(inert.hint().contains("instead of"), "{}", inert.hint());
            }
        }
    }
}
