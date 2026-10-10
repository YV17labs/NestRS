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
    /// [`register`](Discoverable::register) can build this one. Empty for
    /// providers built lazily (controllers, resolvers).
    fn dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject]` dependency, recorded for the access-graph
    /// check whatever the build timing.
    fn injected() -> Vec<TypeId> {
        Vec::new()
    }

    /// Label for each [`injected`](Discoverable::injected) entry, in the same
    /// order, for boot errors; may be shorter than `injected()`, never longer.
    fn injected_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject] Option<Arc<…>>` dependency, which a
    /// [`Net`](crate::Net) around the provider follows.
    fn injected_optional() -> Vec<TypeId> {
        Vec::new()
    }

    /// [`ProviderKey`](crate::ProviderKey) of each **keyed** `#[inject(key = "…")]`
    /// dependency, validated against the global keyed set (seeds + factory
    /// outputs).
    fn injected_keyed() -> Vec<KeyedDependency> {
        Vec::new()
    }

    /// Human-readable label for each [`dependencies`](Discoverable::dependencies)
    /// entry, in the same order, so the boot-time fixpoint can name a missing
    /// dependency.
    fn dependency_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject] Option<Arc<…>>` optional dependency, to
    /// order the provider after one the same module supplies.
    fn optional_dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// Container keys this provider registers **besides itself**, each with the
    /// label a boot error should use for it — so the access graph attributes a
    /// singleton installed on the module's behalf (`nest-rs-ws`'s `WsServer<N>`)
    /// to that module.
    fn also_provides() -> Vec<(TypeId, &'static str)> {
        Vec::new()
    }

    /// Install this provider's construction into the builder — the register
    /// phase's per-provider step, emitted by the decorator.
    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}

/// What the container holds under a provider's **own type**, stated by whatever
/// decorator built it — and read by the impl halves that resolve their host
/// there (`#[hooks]`, `#[scheduled]`, `#[listeners]`, `#[indicators]`,
/// `#[processor]`).
///
/// Those five resolve with `Container::get::<Host>()` outside any request,
/// which answers correctly only for **a singleton stored under its own type**:
///
/// | Host | What `get` does | Symptom if it were allowed |
/// |---|---|---|
/// | edge host (`#[controller]`, `#[gateway]`, `#[resolver]`, `#[mcp]`) | `None` — the type registers *metadata*, an instance is built at mount | the edge serves, the method never runs, one `warn` |
/// | `#[injectable(scope = request)]` | `None` — the container holds a factory, not a value | the same `warn`, misnaming the cause |
/// | `#[injectable(scope = transient)]` | builds a **throwaway** instance | the method runs, its effects are dropped, and nothing warns at all |
///
/// Every decorator that builds a provider writes this impl, `true` or `false`,
/// so contradicting it is a coherence error. A provider registered by hand with
/// [`ContainerBuilder::provide`] writes its own:
///
/// ```
/// # use nest_rs_core::{Container, ProviderResidency};
/// # struct MyHandWrittenProvider;
/// impl ProviderResidency for MyHandWrittenProvider {
///     const SINGLETON: bool = true;
/// }
/// # let container = Container::builder().provide(MyHandWrittenProvider).build();
/// # assert!(container.get::<MyHandWrittenProvider>().is_some());
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
/// the booted container does not hold, and the cause is unknown — the run-time
/// half ([`unresolved_host`]), or a container built without a composition.
///
/// It names causes and prescribes nothing: listing a `dyn`-bound host under its
/// own type too builds it twice, each decorator firing on an instance no
/// `Arc<dyn Trait>` consumer holds.
pub const INERT_HOST_HINT: &str = concat!(
    "nothing is registered under that exact type. Common causes: its module is not imported ",
    "by this app (import it, or delete the methods); it is bound only as `dyn Trait`; it is ",
    "imported through a `for_root`; it is registered by hand under a key or a trait",
);

/// Why a discovered host is inert in this app — which decides the level the
/// boot says it at and the remedy it names. Read by
/// [`report_inert_host!`](crate::report_inert_host!), from [`inert_host`].
///
/// A host a library crate holds and this app does not import is another app's,
/// said at `debug`; the app's own is said at `warn`.
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
    /// nothing under its own type: registered by nothing, or under a trait —
    /// which the boot cannot tell apart.
    NotListed,
    /// Held under its own type by this app's container, and listed by no module
    /// it imports: registered by hand or through a `for_root`.
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

    /// The remedy the cause has; [`INERT_HOST_HINT`]'s list of causes when it
    /// is unknown.
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
/// Without a [`Composition`] — a container built by hand in a test — the cause
/// is [`Unknown`](InertHost::Unknown).
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
/// which the app's container holds under its own type when `held`.
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
/// A `nest-rs-*` crate's opt-in providers are inert whenever the app did not
/// import their module, which is no mistake, so they report at `debug`.
///
/// **Known limit:** any crate named `nest_rs_…`, a third-party plugin
/// included, reads as the framework's.
pub fn is_framework_owned(origin: &str) -> bool {
    let krate = origin.split("::").next().unwrap_or(origin);
    krate == "nest_rs" || krate.starts_with("nest_rs_")
}

/// Report a discovered operation whose host the booted container does not hold,
/// at the level [`inert_host`] decides — `warn` for the app's own, `debug`
/// otherwise — with a `cause` and a `hint`.
///
/// A macro because `tracing` needs `target:` const at the call site; its paths
/// are `$crate::tracing::`, since the expansion resolves in the caller's crate.
///
/// ```
/// # use std::any::TypeId;
/// # use nest_rs_core::{Container, report_inert_host};
/// # struct Entry { origin: &'static str, provider: &'static str, method: &'static str, provider_type_id: fn() -> TypeId }
/// # struct Reports;
/// # let entry = Entry { origin: "my_app::reports", provider: "Reports", method: "nightly", provider_type_id: TypeId::of::<Reports> };
/// # let container = &Container::default();
/// report_inert_host!(
///     target: nest_rs_core::target::LIFECYCLE,
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
        for origin in [
            "features::users::service",
            "api::module",
            "my_nest_rs_helpers::hooks",
        ] {
            assert!(!is_framework_owned(origin), "{origin}");
        }
    }

    /// Pins the known limit of [`is_framework_owned`].
    #[test]
    fn a_third_party_crate_taking_the_prefix_is_read_as_the_frameworks() {
        assert!(is_framework_owned("nest_rs_audit_probe"));
    }

    /// Guards the shape, not one wording: the forbidden remedy cannot be written
    /// without naming a `providers` list or an imperative.
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
        use crate::access::__private::{ModuleDescriptor, ProviderDescriptor};
        use crate::access::Composition;
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
                        injects_optional: nothing,
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
                        injects_optional: nothing,
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
