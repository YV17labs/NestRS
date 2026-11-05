//! Every error the boot can fail with, and the two the kernel lends every edge
//! and layer rather than raises: [`DecodeError`] and [`MissingAmbientError`].

use std::borrow::Cow;

use serde_json::error::Category;
use thiserror::Error;

use crate::container::ProviderScope;
use crate::type_name::shorten;

/// A provider depends on something its module does not import and that is not
/// global infrastructure. Raised at boot by the access-graph validation.
///
/// The boot builds it; a program reads its fields, and a later 7.x may add one:
///
/// ```compile_fail,E0639
/// let refused = nest_rs_core::AccessGraphError {
///     module: "UsersModule",
///     consumer: "UsersService",
///     dependency: "Mailer",
///     owner: "MailModule",
/// };
/// ```
#[derive(Debug, Error)]
#[error(
    "module access violation: `{consumer}` (in module `{module}`) depends on `{dependency}`, \
     but `{module}` imports no module that provides it. `{dependency}` is provided by `{owner}` \
     — add `{owner}` to `#[module(imports = [...])]` of `{module}`, or route the dependency \
     through a module `{module}` already imports."
)]
#[non_exhaustive]
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

/// A layer required a value no earlier layer attached to its unit
/// ([`UnitContext::require`](crate::UnitContext::require)): a wiring defect —
/// the guard that attaches it is not bound, or runs after the one requiring
/// it — never the caller's doing, but for the principal a guard declares
/// expecting, which the boot proves attached. A guard's `?` answers it as a `500`
/// denial, filed once at `error` by the edge, naming the guard and the value.
#[derive(Debug, Error)]
#[error(
    "no `{}` is ambient on this unit: the layer that attaches it is not bound before the one \
     requiring it",
    shorten(.value)
)]
#[non_exhaustive]
pub struct MissingAmbientError {
    /// The type of the value required, as `std::any::type_name` spells it.
    pub value: &'static str,
    /// That type's identity, which a guard's declared principal compares.
    pub type_id: std::any::TypeId,
}

impl MissingAmbientError {
    /// A `T` was required and is not ambient — what
    /// [`UnitContext::require`](crate::UnitContext::require) answers, for a
    /// layer that reads `T` another way first.
    pub fn of<T: 'static>() -> Self {
        Self {
            value: std::any::type_name::<T>(),
            type_id: std::any::TypeId::of::<T>(),
        }
    }
}

/// A provider depends on something **no module provides** — not global
/// infrastructure, not in its import closure, not registered anywhere. Raised
/// at boot, for lazily- and eagerly-built providers alike.
///
/// When the crate owning the dependency's type names the line to add — a
/// [`DependencyRemedy`](crate::DependencyRemedy), `RedisQueueModule` for the
/// queue port's `Jobs` — the refusal says that line in place of the
/// kernel's generic one:
///
/// ```
/// let refused = nest_rs_core::MissingDependencyError::new("PostsModule", "PostsService", "Jobs")
///     .with_remedy("Import exactly one queue backend's binding.");
/// assert_eq!(
///     refused.to_string(),
///     "unmet dependency: `PostsService` (in module `PostsModule`) depends on `Jobs`, \
///      which no module this app imports provides. Import exactly one queue backend's binding.",
/// );
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub struct MissingDependencyError {
    /// Module that owns the consumer whose dependency is unmet.
    pub module: &'static str,
    /// Provider whose dependency no module supplies.
    pub consumer: &'static str,
    /// The dependency that is registered nowhere and is not global infra.
    pub dependency: &'static str,
    /// The line to add, as the crate owning the dependency's type names it —
    /// `None` when no crate names one, and the refusal says the generic remedy.
    pub remedy: Option<&'static str>,
}

impl MissingDependencyError {
    /// `consumer`, in `module`, needs `dependency`, which nothing provides — as
    /// a module's factory says of what it reads from the container.
    pub const fn new(
        module: &'static str,
        consumer: &'static str,
        dependency: &'static str,
    ) -> Self {
        Self {
            module,
            consumer,
            dependency,
            remedy: None,
        }
    }

    /// The same refusal, naming `remedy` — the line the dependency's owning
    /// crate prescribes — in place of the generic one.
    pub const fn with_remedy(self, remedy: &'static str) -> Self {
        Self {
            remedy: Some(remedy),
            ..self
        }
    }
}

impl std::fmt::Display for MissingDependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            module,
            consumer,
            dependency,
            remedy,
        } = self;
        write!(
            f,
            "unmet dependency: `{consumer}` (in module `{module}`) depends on `{dependency}`, "
        )?;
        match remedy {
            Some(remedy) => write!(f, "which no module this app imports provides. {remedy}"),
            None => write!(
                f,
                "but no module provides it and it is not global infrastructure (a seed or \
                 factory output). Add a provider for `{dependency}` to a module reachable from \
                 the root, or seed it at `App::builder()`."
            ),
        }
    }
}

/// A **singleton** provider `#[inject]`s a `#[injectable(scope = request)]` or
/// `scope = transient` provider, which is built per request or per
/// resolution, never once at boot where the singleton is built. The remedy
/// is a handler's [`Scoped<T>`](crate::Scoped) parameter.
#[derive(Debug, Error)]
#[error(
    "scope violation: `{consumer}` (in module `{module}`) is a singleton and injects \
     `{dependency}`, which is request-scoped or transient: `{consumer}` is built once, at boot, \
     where no request exists to build `{dependency}` for it. Reach `{dependency}` through a \
     `Scoped<T>` parameter of the handler the edge dispatches, or make `{consumer}` \
     request-scoped too."
)]
#[non_exhaustive]
pub struct ScopeViolationError {
    /// Module that owns the offending consumer.
    pub module: &'static str,
    /// The singleton provider whose `#[inject]` cannot be honoured.
    pub consumer: &'static str,
    /// The request-scoped or transient dependency it named.
    pub dependency: &'static str,
}

/// A layer bound where it is resolved once, at boot — on a provider-hosted
/// site (a `#[process]` method's, a trigger's, an `#[on_event]` method's) or in
/// an app-wide pool (`GlobalGuards<(…)>` and its siblings) — is
/// request-scoped or transient: a request-scoped layer is never built there,
/// and a transient one would be built once and shared by every run, which is
/// not transient.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "scope violation: `{layer}` is bound by `{host}` (in module `{module}`), which resolves its \
     layers once, at boot, and is request-scoped or transient — a request-scoped `{layer}` \
     would never be built there, and a transient one would be built once and shared by every \
     run. Make `{layer}` a singleton."
)]
pub struct ScopedLayerError {
    /// Module that lists the host.
    pub module: &'static str,
    /// What binds the layer: the provider hosting the site, or the app-wide
    /// pool's declaration.
    pub host: &'static str,
    /// The request-scoped or transient layer.
    pub layer: &'static str,
}

/// A link-time registry was read from a container no app sealed: one built by
/// hand carries no reachability set, and discovery never runs ungated.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "{reader}s are discovered from a container that carries no reachability set; build it \
     through App::builder, whose boot seals the set its modules reach"
)]
pub struct UngatedDiscoveryError {
    /// What the reader discovers, as its inert-host report names it.
    pub reader: &'static str,
}

/// The failure modes of the access-graph pass: a cross-module reach that no
/// import covers, or a dependency no module provides.
///
/// `pub(crate)`: [`into_anyhow`](Self::into_anyhow) discards it before a boot
/// failure leaves the crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub(crate) enum AccessError {
    /// A provider reached across modules for something no import covers.
    #[error(transparent)]
    CrossModule(#[from] AccessGraphError),
    /// A provider depends on something no module provides at all.
    #[error(transparent)]
    Missing(#[from] MissingDependencyError),
    /// A singleton injected a provider that only exists inside a request.
    #[error(transparent)]
    Scope(#[from] ScopeViolationError),
    /// A provider-hosted site bound a layer that only exists inside a request.
    #[error(transparent)]
    ScopedLayer(#[from] ScopedLayerError),
}

impl AccessError {
    /// Flatten into an `anyhow::Error` carrying the **concrete** inner error,
    /// so a boot failure downcasts to it directly; boxing to `dyn Error` first
    /// would lose the downcast.
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        match self {
            AccessError::CrossModule(e) => anyhow::Error::new(e),
            AccessError::Missing(e) => anyhow::Error::new(e),
            AccessError::Scope(e) => anyhow::Error::new(e),
            AccessError::ScopedLayer(e) => anyhow::Error::new(e),
        }
    }
}

/// What [`ContainerBuilder::build`](crate::ContainerBuilder::build) refuses a
/// container built by hand with: the first refusal the builder holds, read in
/// the boot's order — a contest, a seed its factory's check refuses, what its
/// register phase left, then what the seal reads off its modules. The access
/// graph is the boot's alone: it holds a provider
/// to what is global, a seed or a factory's output, which a builder made by
/// hand cannot tell from what its modules register, both being calls on it.
/// So are the wirings, which fill their registries from the reachable set the
/// boot's seal hands the container.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContainerError {
    /// Two import sites declared a value for one type.
    #[error(transparent)]
    ContestedDeclaration(#[from] ContestedDeclarationError),
    /// A seed of a factory's output its module's check refuses, as a value
    /// the factory read would be refused.
    #[error(transparent)]
    SeedRefused(anyhow::Error),
    /// What a `register` filed with
    /// [`ContainerBuilder::refuse`](crate::ContainerBuilder::refuse), as it
    /// filed it.
    #[error(transparent)]
    Refused(anyhow::Error),
    /// A factory queued that nothing drains: the module whose `collect` queued
    /// it was left out, or it was queued on the builder itself.
    #[error(transparent)]
    LateFactory(#[from] LateFactoryError),
    /// Providers waiting on each other: singletons of one module or of modules
    /// importing each other, or request-scoped and transient ones.
    #[error(transparent)]
    ProviderCycle(#[from] ProviderCycleError),
    /// A module imported in a collect phase alone.
    #[error(transparent)]
    UnregisteredModule(#[from] UnregisteredModuleError),
    /// A type registered twice.
    #[error(transparent)]
    DuplicateProvider(#[from] DuplicateProviderError),
    /// A provider a module lists, never built.
    #[error(transparent)]
    UnbuiltProvider(#[from] UnbuiltProviderError),
    /// A provider built without an optional dependency registered after it.
    #[error(transparent)]
    LateOptionalDependency(#[from] LateOptionalDependencyError),
    /// A resource's budget at or past the net of a port that waits on it.
    #[error(transparent)]
    BudgetPastNet(#[from] BudgetPastNetError),
}

impl ContainerError {
    /// Flatten into an `anyhow::Error` carrying the **concrete** refusal, as
    /// every boot failure does, so it downcasts to it directly.
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::ContestedDeclaration(e) => anyhow::Error::new(e),
            Self::SeedRefused(e) => e,
            Self::Refused(e) => e,
            Self::LateFactory(e) => anyhow::Error::new(e),
            Self::ProviderCycle(e) => anyhow::Error::new(e),
            Self::UnregisteredModule(e) => anyhow::Error::new(e),
            Self::DuplicateProvider(e) => anyhow::Error::new(e),
            Self::UnbuiltProvider(e) => anyhow::Error::new(e),
            Self::LateOptionalDependency(e) => anyhow::Error::new(e),
            Self::BudgetPastNet(e) => anyhow::Error::new(e),
        }
    }
}

/// A type registered twice — a concrete type or a trait object, under any two
/// scopes; names both registrations and, when they differ, their scopes. A
/// test override is never one of the two: what would register its type stands
/// down for it.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "duplicate provider: `{}` is registered by {first} and again by {second}{} — one type has \
     one provider: remove one registration; a test replaces a provider with `override_value`, \
     `override_arc` or `override_dyn`",
    shorten(.type_name),
    scopes_clause(*.scopes)
)]
pub struct DuplicateProviderError {
    /// The type registered twice, as `std::any::type_name` spells it; the
    /// sentence says it without its module paths.
    pub type_name: &'static str,
    /// The registration made first: an import site, a seed, or a factory and
    /// the site that queued it.
    pub first: String,
    /// The registration that repeated it, the same way.
    pub second: String,
    /// The two scopes, first then second, when they differ.
    pub scopes: Option<(ProviderScope, ProviderScope)>,
}

/// ` (first as a singleton, then request-scoped)` when two registrations hold
/// one type under two scopes; nothing otherwise.
fn scopes_clause(scopes: Option<(ProviderScope, ProviderScope)>) -> String {
    match scopes {
        Some((first, second)) => format!(
            " (first {}, then {})",
            first.as_registered(),
            second.as_registered()
        ),
        None => String::new(),
    }
}

/// Two import sites each *declared* a value for the same type, or attached the
/// same transport type, and one of them would have to lose.
///
/// What contests: a **declaration** (`ContainerBuilder::provide_declared_factory`,
/// or the trait object a `provide_factory_dyn` binds), and a **transport type**
/// attached twice (`ContainerBuilder::provide_transport`: an app runs one of
/// each). The same default or binding queued twice never does, so a diamond
/// import stays legal. Both are named by their import site; `remedy` comes from
/// the declaring seam.
///
/// Raised before any factory runs for what the imports' `collect` declared,
/// and once the register phase ends for what a `register` declared or
/// attached.
#[derive(Debug, Error)]
#[error(
    "contested declaration: `{}` is declared twice — by {first}, and by {second}. {remedy}",
    shorten(.type_name)
)]
#[non_exhaustive]
pub struct ContestedDeclarationError {
    /// The type declared more than once, as `std::any::type_name` spells it;
    /// the sentence says it without its module paths.
    pub type_name: &'static str,
    /// The declaration made first, as the import that made it.
    pub first: String,
    /// The declaration that contested it, the same way.
    pub second: String,
    /// What the reader should do instead, supplied by the declaring seam.
    pub remedy: &'static str,
}

/// Every factory left in the queue waits on a factory output still in it — one
/// another's, or its own — so none can run first: a cycle in the `*_after`
/// declarations.
#[derive(Debug, Error)]
#[error(
    "factory cycle: {} — each waits on a factory output that a member of this set (itself \
     included) would provide, so no order satisfies them. Drop one `_after` declaration.",
    and_list(&.type_names.iter().map(|name| shorten(name)).collect::<Vec<_>>())
)]
#[non_exhaustive]
pub struct FactoryCycleError {
    /// The types whose factories wait on each other.
    pub type_names: Vec<&'static str>,
}

/// A factory failed, so the boot ends before any provider is built from it:
/// named by the type it would have built, its cause the factory's own error.
///
/// A factory that reads its value rather than builds it — a `#[config]` from
/// the environment — fails the boot with its own refusal instead, which names
/// the variable an operator sets.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error("the factory for `{}` failed", shorten(.type_name))]
pub struct FactoryFailedError {
    /// The type the factory would have built, as `std::any::type_name`
    /// spells it; the sentence says it without its module paths.
    pub type_name: &'static str,
    /// What the factory failed with.
    #[source]
    pub source: anyhow::Error,
}

/// A stop was asked while the boot waited on one of its steps: the step was
/// dropped where it waited, and the app never served.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error("the boot was stopped by {} during {step}", .cause.described())]
pub struct BootInterruptedError {
    /// Why the app stops.
    pub cause: crate::StopCause,
    /// The step the stop interrupted.
    pub step: crate::BootStep,
}

/// A transport states a stop past [`MAX_STOP_BOUND`](crate::MAX_STOP_BOUND) —
/// [`Duration::MAX`](std::time::Duration::MAX) among them —, refused at the
/// boot before any init hook runs: the kernel holds every transport to its
/// bound.
#[derive(Debug, Error)]
#[error(
    "transport `{transport}` states a stop bound of {stop_bound_ms} ms, past the one-day \
     ceiling: a transport's stop is bounded: declare its drain window"
)]
#[non_exhaustive]
pub struct UnboundedTransportError {
    /// The transport, by its [`name`](crate::Transport::name).
    pub transport: String,
    /// The bound it states, in milliseconds, saturated at `u64::MAX`.
    pub stop_bound_ms: u64,
}

/// The deployment sets an OTLP endpoint for a signal it leaves on, and the
/// process logs through the kernel's console alone, which exports nothing:
/// refused by [`App::run`](crate::App::run) before any transport is built,
/// naming the variable and the line that installs the exporter. A suite,
/// which boots without `run`, is never refused.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "{variable} sets an OTLP endpoint, but nothing exports to it: the process logs through the \
     kernel's console alone — add `let _otel = nest_rs::opentelemetry::OpenTelemetry::init()?;` \
     at the top of `main`, with the umbrella's `opentelemetry` feature, or set \
     OTEL_SDK_DISABLED=true where this binary exports nothing"
)]
pub struct UnservedExportError {
    /// The variable that sets the endpoint.
    pub variable: &'static str,
}

/// The app's init phases failed, were interrupted, or were dropped before they
/// finished, on an earlier call: they do not run again, so neither
/// [`App::init`](crate::App::init) nor [`App::run`](crate::App::run) proceeds.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error("the app's init failed earlier: it does not run again")]
pub struct InitFailedError;

/// A module queued an async factory during the register phase, once the
/// collect phase had ended, so no boot drains it. A default whose output is
/// already present is discarded instead; a declaration is refused all the same.
///
/// The usual cause is a module or [`DynamicModule`](crate::DynamicModule)
/// importing another in its `register` alone: that module is collected there,
/// and refused before its `register` runs. A container built by hand
/// ([`Container::builder`](crate::Container::builder)) is in the register
/// phase from the start and drains no factory, so its sentence prescribes
/// that builder's edit: a seed for a default, the boot for a declaration.
#[derive(Debug, Error)]
#[error("{}", late_factory_sentence(&shorten(.type_name), *.queued_on))]
pub struct LateFactoryError {
    /// The type whose factory was queued too late to run.
    pub type_name: &'static str,
    /// The builder it was queued on, which decides the edit.
    queued_on: QueuedOn,
}

/// The builder a [`LateFactoryError`]'s factory was queued on.
#[derive(Clone, Copy, Debug)]
enum QueuedOn {
    /// A boot's, in its register phase.
    Boot,
    /// A container built by hand, which drains no factory, as the kind of
    /// factory queued.
    Hand(FactoryKind),
}

/// The kind of a factory a container built by hand never runs, which decides
/// what stands in for it there.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FactoryKind {
    /// A default, which a seed of its value answers.
    Default,
    /// A default binding trait objects off its value, which a seed of the value
    /// alone leaves absent.
    Binding,
    /// A value an import site chose, which no seed stands in for.
    Declaration,
}

impl LateFactoryError {
    /// `type_name`'s factory, queued during a boot's register phase.
    pub(crate) fn in_boot(type_name: &'static str) -> Self {
        Self {
            type_name,
            queued_on: QueuedOn::Boot,
        }
    }

    /// `type_name`'s factory of `kind`, queued on a container built by hand.
    pub(crate) fn by_hand(type_name: &'static str, kind: FactoryKind) -> Self {
        Self {
            type_name,
            queued_on: QueuedOn::Hand(kind),
        }
    }
}

/// What a [`LateFactoryError`] says of `type_name`'s factory, queued on `on`.
fn late_factory_sentence(type_name: &str, on: QueuedOn) -> String {
    match on {
        QueuedOn::Boot => format!(
            "`{type_name}` is provided by an async factory queued during the register phase, \
             which no boot drains. Queue it in `collect` — a module imported in its importer's \
             `register` alone collects there, so import it through `ContainerBuilder::import` in \
             that importer's `collect` too."
        ),
        QueuedOn::Hand(FactoryKind::Declaration) => format!(
            "`{type_name}` is declared by an async factory — a value an import site chose, a \
             module's `for_root` value among them — which a container built by hand never \
             runs, and a value provided there never stands in for a declaration: boot through \
             `App::builder()`, `TestApp` in a test, which runs it."
        ),
        QueuedOn::Hand(kind) => format!(
            "`{type_name}` is provided by an async factory, which a container built by hand \
             never runs: provide `{type_name}`{} on the builder before what queues it — a \
             `#[config]` a module's `collect` queues among them — or boot through \
             `App::builder()`, `TestApp` in a test, which runs it.",
            if matches!(kind, FactoryKind::Binding) {
                ", and each trait object its factory binds off it (`provide_dyn`),"
            } else {
                ""
            }
        ),
    }
}

/// A module was imported in a collect phase and in no register phase, so none
/// of its providers registered. The cause is a module or
/// [`DynamicModule`](crate::DynamicModule) importing it in its `collect` alone.
#[derive(Debug, Error)]
#[error(
    "`{module}` was imported in a collect phase alone, so none of its providers registered: \
     import it through `ContainerBuilder::import` in its importer's `register` too."
)]
#[non_exhaustive]
pub struct UnregisteredModuleError {
    /// The module imported in a collect phase alone.
    pub module: &'static str,
}

/// A `#[module]`'s dynamic import reached the register phase with no value its
/// collect phase built — only a hand call of the expansion's internal seam.
#[derive(Debug, Error)]
#[error(
    "{site} reached the register phase with no value its module's collect built — the import's \
     two phases ran apart, which only a hand call of `#[module]`'s internal seam does"
)]
pub(crate) struct UncollectedImportError {
    /// The import, as a boot error names its site.
    pub(crate) site: String,
}

/// Providers waiting on each other, so none can be built first: singletons of
/// one module, refused by the register phase that cannot order them, or
/// request-scoped and transient providers, refused at the seal before a
/// request would resolve them.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "module `{module}`: {} provider cycle {} — {CYCLE_REMEDY}",
    .scope.as_adjective(),
    arrow_chain(.chain)
)]
pub struct ProviderCycleError {
    /// The module of the chain's first provider.
    pub module: &'static str,
    /// The scope of the chain's first provider.
    pub scope: ProviderScope,
    /// The providers in the order each waits on the next, the first repeated
    /// last: `["A", "B", "A"]`.
    pub chain: Vec<&'static str>,
}

/// The edit every provider cycle has, whichever scope and wherever it is
/// caught.
pub(crate) const CYCLE_REMEDY: &str = "break it by moving what both need into a third provider \
     both inject, by passing the value as a method argument, or by publishing an event one side \
     listens to";

/// What a resolution that closes a cycle among providers registered by hand —
/// which no descriptor shows the boot — panics with.
pub(crate) fn cycle_panic_message(scope: ProviderScope, chain: &str) -> String {
    format!(
        "{} provider cycle: {chain} — {CYCLE_REMEDY}",
        scope.as_adjective()
    )
}

/// `` `A` → `B` → `A` ``.
fn arrow_chain(chain: &[&str]) -> String {
    let quoted: Vec<String> = chain.iter().map(|name| format!("`{name}`")).collect();
    quoted.join(" → ")
}

/// A provider a `#[module]` lists that the register phase never built: what it
/// injects was not registered yet when its module's providers were built.
/// Raised once the access graph passes, which names a missing dependency's
/// owner more precisely; on a container built by hand, which has no access
/// graph, by [`ContainerBuilder::build`](crate::ContainerBuilder::build).
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "module `{module}`: `{provider}` was never built — it needs {}, which nothing had registered \
     when `{module}`'s providers were built; {}",
    and_list(.missing),
    unbuilt_clause(.cause, .module, .missing.len())
)]
pub struct UnbuiltProviderError {
    /// The module listing the provider.
    pub module: &'static str,
    /// The provider never built.
    pub provider: &'static str,
    /// What it injects that was absent when it would have been built.
    pub missing: Vec<&'static str>,
    /// Why it was absent.
    pub cause: UnbuiltCause,
}

/// Why a dependency was absent when its consumer's module built its providers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnbuiltCause {
    /// The modules of an import cycle, the consumer's first: one of them
    /// registers before the other's providers exist.
    ImportCycle(Vec<&'static str>),
    /// Registered later in the boot, by a hand-written module or a `for_root`
    /// the consumer's module does not import.
    RegisteredLater,
    /// The consumer's module was imported on a container built by hand while
    /// nothing there provided it: the builder runs its calls in the order they
    /// are written.
    ImportedFirst,
    /// Only an app-wide pool's declaration lists it, and a pool registers what
    /// it lists once every module is built.
    RegisteredByAPool,
}

/// A provider built without an optional dependency the same boot registered
/// afterwards: it would run as if the capability were absent. Raised once the
/// access graph passes, and by
/// [`ContainerBuilder::build`](crate::ContainerBuilder::build) on a container
/// built by hand.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "module `{module}`: `{provider}` was built without its optional `{dependency}`, registered \
     only afterwards, so it would run as if the capability were absent; {}",
    unbuilt_clause(.cause, .module, 1)
)]
pub struct LateOptionalDependencyError {
    /// The module listing the provider.
    pub module: &'static str,
    /// The provider built without it.
    pub provider: &'static str,
    /// The optional dependency registered after the provider was built.
    pub dependency: &'static str,
    /// Why it was absent then.
    pub cause: UnbuiltCause,
}

/// What an [`UnbuiltCause`] says of `count` dependencies of a provider in
/// `module`: the cause, then its edit.
fn unbuilt_clause(cause: &UnbuiltCause, module: &str, count: usize) -> String {
    let them = if count == 1 { "it" } else { "them" };
    match cause {
        UnbuiltCause::ImportCycle(modules) if modules.len() == 2 => format!(
            "{} import each other, so one registers before the other's providers exist: move \
             what both need into a module both import",
            and_list(modules)
        ),
        UnbuiltCause::ImportCycle(modules) => format!(
            "{} import each other in a cycle, so one registers before another's providers \
             exist: move what they need into a module they all import",
            and_list(modules)
        ),
        UnbuiltCause::RegisteredLater => format!(
            "a hand-written module or a `for_root` outside `{module}`'s imports registers {them} \
             later: import that module in `{module}`'s `imports`"
        ),
        UnbuiltCause::RegisteredByAPool => format!(
            "only an app-wide pool's declaration lists {them}, and a pool registers what it \
             lists once every module is built: list {them} in `{module}`'s `providers`"
        ),
        UnbuiltCause::ImportedFirst => format!(
            "`{module}` was imported on a container built by hand while nothing there \
             provided {them}: provide {them}, or import what provides {them}, before \
             `{module}`"
        ),
    }
}

/// A test override nothing in the app's graph would register: no module lists
/// a provider the container holds under that type, and no factory or seed
/// makes it, so the double would never be read. Raised once the register phase
/// ends.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "the test overrides `{}`, which nothing in this app's graph provides — no module lists it, \
     and no factory or seed makes it — so the double would never be read: override the type \
     the app registers (for a `Foo as dyn Trait` binding, `override_dyn::<dyn Trait>`)",
    shorten(.type_name)
)]
pub struct UnclaimedOverrideError {
    /// The overridden type.
    pub type_name: &'static str,
}

/// A provider injects a key a test override removed: the override stands in
/// for a whole provider through one of its keys, so the provider is never
/// built and its other keys are absent. Raised once the access graph passes.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "unmet dependency: `{consumer}` (in module `{module}`) injects `{dependency}`, which the \
     test removed with its provider — `{provider}` is overridden through `{}`: override the key \
     this consumer injects",
    shorten(.through)
)]
pub struct OverriddenDependencyError {
    /// The module listing the consumer.
    pub module: &'static str,
    /// The provider injecting the removed key.
    pub consumer: &'static str,
    /// The removed key, as the consumer labels it.
    pub dependency: &'static str,
    /// The overridden provider, by its own type.
    pub provider: &'static str,
    /// The key the test overrode it through.
    pub through: &'static str,
}

/// `` `A` ``, `` `A` and `B` ``, `` `A`, `B` and `C` ``.
fn and_list<S: AsRef<str>>(names: &[S]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("`{}`", only.as_ref()),
        [init @ .., last] => {
            let init: Vec<String> = init
                .iter()
                .map(|name| format!("`{}`", name.as_ref()))
                .collect();
            format!("{} and `{}`", init.join(", "), last.as_ref())
        }
    }
}

/// A resource's [`Budget`](crate::Budget) at or past the [`Net`](crate::Net) of
/// a port that waits on it: the port would cut a call the resource was still
/// answering, replacing its cause with a bare timeout.
#[derive(Debug, Error)]
#[error(
    "{resource}'s budget ({budget:?}) must be shorter than {port}'s net ({net:?}), which would \
     otherwise give up on a call still answering and lose its cause: lower {setting}"
)]
#[non_exhaustive]
pub struct BudgetPastNetError {
    /// The resource, as a sentence names it.
    pub resource: &'static str,
    /// What the resource waits, at most, for one answer.
    pub budget: std::time::Duration,
    /// The port whose net reaches the resource, as a sentence names it.
    pub port: &'static str,
    /// What the port waits before it gives up.
    pub net: std::time::Duration,
    /// What lowers the budget: the variable, and the field that pins it in code.
    pub setting: String,
}

/// A wiring a module attaches at the kernel's wiring step failed: the boot ends
/// before any transport is built or any lifecycle hook runs, its cause the
/// wiring's own error.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error("the `{registry}` wiring failed")]
pub struct WiringFailedError {
    /// The registry the wiring fills, as its module names it.
    pub registry: &'static str,
    /// What the wiring failed with.
    #[source]
    pub source: anyhow::Error,
}

/// A layer declared on a site — a route, a gateway's upgrade or message, an
/// operation — whose provider no imported module registers, so the site would
/// otherwise run without it.
#[derive(Debug, Error)]
#[error(
    "`{}` is declared on `{site}`, and no imported module provides it — import the module that \
     provides it",
    shorten(.layer)
)]
#[non_exhaustive]
pub struct UnresolvedLayerError {
    /// The layer type, as declared.
    pub layer: &'static str,
    /// The site declaring it, as its boot log names it.
    pub site: String,
}

/// Two `#[meta]` values of one type declared at one level of a site — on its
/// method, or on its host — which no read could tell apart: a read takes the
/// first value of a type, so the second would be read by nothing.
///
/// The decorator refuses the pair at compile time when each expression names
/// its type; this refuses the rest, by the type the values resolve to.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error(
    "`#[meta]` declares two `{}` values on `{declared_on}` — a read takes the first value of a \
     type, so the second would be read by nothing: keep one; a method's own value already \
     overrides its host's",
    shorten(.type_name)
)]
pub struct DuplicateMetaError {
    /// The values' type.
    pub type_name: &'static str,
    /// Where both are declared: the site, for its method's values, or the
    /// host.
    pub declared_on: String,
}

/// A payload that did not decode, as every edge reports it: the category of the
/// failure, the line and column when the payload was text, the kind of value
/// found and the type expected — never the value.
///
/// serde's own sentences quote the payload (``invalid type: string
/// "sk_live_…", expected u64``), so the sentence is rebuilt from the shapes
/// serde words; any other — a type's own `custom` message — is reported by its
/// category alone. A key or variant the payload spelled is dropped like a
/// value, and what is kept is bounded ([`MAX_LEN`](Self::MAX_LEN)).
///
/// Built with `DecodeError::from(&error)` from serde_json's error or serde's
/// [`value::Error`](serde::de::value::Error) (`serde_urlencoded`); a text
/// already rendered from an error goes through [`redact`](Self::redact). An
/// edge answering the failure [`located`](Self::located) it first, and says
/// its [`path`](Self::path) before the report.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{sentence}{}", .position.as_deref().unwrap_or_default())]
pub struct DecodeError {
    sentence: String,
    position: Option<String>,
    path: Option<String>,
    /// A failure of the value's type, which a tracked re-run can place; a
    /// syntax failure's line and column already say where.
    of_data: bool,
}

impl DecodeError {
    /// The longest a report runs before its position, in bytes; past it the
    /// sentence is cut at a character boundary and ends in `…`.
    pub const MAX_LEN: usize = 512;

    /// The report of `error`, without the value it quoted.
    pub fn new(error: &serde_json::Error) -> Self {
        let rendered = error.to_string();
        let position = (error.line() > 0)
            .then(|| format!(" at line {} column {}", error.line(), error.column()));
        let message = position
            .as_deref()
            .and_then(|position| rendered.strip_suffix(position))
            .unwrap_or(&rendered);
        let of_data = error.classify() == Category::Data;
        let sentence = if of_data {
            data(message)
        } else {
            // One fixed sentence per fault, naming no byte of the input.
            message.to_owned()
        };
        Self {
            sentence: bounded(&sentence).into_owned(),
            position,
            path: None,
            of_data,
        }
    }

    /// Where in its payload the failure sits, in the names the payload's type
    /// declares: `author.name`, `items[2]`, a map's entry as `limits[…]`.
    /// `None` until the report is [`located`](Self::located), and for a
    /// failure of the payload as a whole — its syntax, its root's type.
    ///
    /// ```
    /// use nest_rs_core::DecodeError;
    ///
    /// #[derive(Debug, serde::Deserialize)]
    /// struct Order {
    ///     quantity: u32,
    /// }
    ///
    /// let body = r#"{"quantity":"sk_live_51"}"#;
    /// let failure = serde_json::from_str::<Order>(body).expect_err("not a number");
    /// let report = DecodeError::new(&failure)
    ///     .located::<Order, _>(&mut serde_json::Deserializer::from_str(body));
    /// assert_eq!(report.path(), Some("quantity"));
    /// assert_eq!(report.to_string(), "invalid type: a string, expected u32");
    /// ```
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// This report, located: `again`, a deserializer over the payload that
    /// failed, runs `T`'s decode once more under a tracker, and the report
    /// says where in `T` it failed — a field by the name `T` declares, an
    /// element by its index, a map's entry as `[…]`, never a key the payload
    /// chose — and, for a type's refusal in its own words, what that type
    /// expected there. Run only once a decode has failed, so a decode that
    /// succeeds pays nothing for it; but the payload must outlive the first
    /// decode, so a `serde_json::Value` is decoded by reference, which copies
    /// the strings `serde_json::from_value` would have moved.
    ///
    /// A field missing or given twice is placed at the field, said as
    /// `missing` or `duplicate`. A syntax failure keeps its line and column,
    /// and a located one drops them: the path says where.
    pub fn located<'de, T, D>(self, again: D) -> Self
    where
        T: serde::Deserialize<'de>,
        D: serde::Deserializer<'de>,
        for<'e> Self: From<&'e D::Error>,
    {
        if !self.of_data {
            return self;
        }
        match crate::decode_path::trace::<T, D>(again) {
            Err((error, failed)) => Self::from(&error).at(failed),
            // A decode that changed its mind has nothing to place.
            Ok(_) => self,
        }
    }

    /// This report placed where `failed` says.
    fn at(mut self, failed: crate::decode_path::Failed) -> Self {
        let crate::decode_path::Failed {
            mut path,
            fields,
            expected,
        } = failed;
        // serde names the field a struct misses, or reads twice, by the name
        // the struct declares: said at the field instead.
        for (opening, said) in [
            ("missing field `", "missing"),
            ("duplicate field `", "duplicate"),
        ] {
            let named = self
                .sentence
                .strip_prefix(opening)
                .and_then(|rest| rest.strip_suffix('`'));
            let declared =
                named.and_then(|name| fields?.iter().copied().find(|field| *field == name));
            if let Some(field) = declared {
                path.0.push(crate::decode_path::Step::Field(field));
                said.clone_into(&mut self.sentence);
            }
        }
        if let Some(expected) = expected.filter(|_| self.sentence == REFUSED_BY_ITS_TYPE) {
            self.sentence =
                bounded(&format!("{REFUSED_BY_ITS_TYPE}, expected {expected}")).into_owned();
        }
        let path = path.to_string();
        self.path = (!path.is_empty()).then(|| bounded(&path).into_owned());
        self.position = None;
        self
    }

    /// The report of the first decode failure in `error`'s chain, `error`
    /// itself first, down to an opaque failure's cause: what an edge answers
    /// for a failure it reads as one.
    ///
    /// ```
    /// use nest_rs_core::DecodeError;
    ///
    /// #[derive(Debug, thiserror::Error)]
    /// #[error("the stored profile is unreadable")]
    /// struct Unreadable(#[source] serde_json::Error);
    ///
    /// let failure = serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
    /// let report = DecodeError::in_chain(&Unreadable(failure)).expect("a decode failure in the chain");
    /// assert_eq!(report.to_string(), "invalid type: a string, expected u64 at line 1 column 16");
    /// ```
    pub fn in_chain(error: &(dyn std::error::Error + 'static)) -> Option<Self> {
        crate::error_message::__private::answerable_links(error).find_map(Self::of)
    }

    /// `link` as the report it is, when it is a decode failure — serde_json's
    /// error, or serde's own value error.
    pub(crate) fn of(link: &(dyn std::error::Error + 'static)) -> Option<Self> {
        if let Some(error) = link.downcast_ref::<serde_json::Error>() {
            return Some(Self::new(error));
        }
        link.downcast_ref::<serde::de::value::Error>()
            .map(Self::from)
    }

    /// `text` said without the values a decode failure quotes in it — a reply's
    /// detail, a frame, any sentence rendered from an error before the edge that
    /// reports it held it.
    ///
    /// Each decode failure in `error`'s chain is replaced by its report wherever
    /// `text` spells it, and each link a [`SafeLink`](crate::SafeLink) claims by
    /// what its driver says of it; then any sentence still in one of serde's
    /// quoting shapes (`invalid type:`, `invalid value:`, ``unknown variant ` ``,
    /// ``unknown field ` ``) is rebuilt without its value, from there to the
    /// end of `text`. Each `validator` `Validation error: <code> [<params>]` is
    /// said as `Validation error: <code>`.
    ///
    /// Borrowed when nothing in `text` was a decode, validation or claimed
    /// failure's.
    pub fn redact<'t>(
        text: &'t str,
        error: Option<&(dyn std::error::Error + 'static)>,
    ) -> Cow<'t, str> {
        Redactions::of(error).redact(text)
    }
}

impl From<serde_json::Error> for DecodeError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(&error)
    }
}

impl From<&serde_json::Error> for DecodeError {
    fn from(error: &serde_json::Error) -> Self {
        Self::new(error)
    }
}

/// serde's own error for a value read through `IntoDeserializer`
/// (`serde_urlencoded`): always a data error, with no position.
impl From<&serde::de::value::Error> for DecodeError {
    fn from(error: &serde::de::value::Error) -> Self {
        Self {
            sentence: bounded(&data(&error.to_string())).into_owned(),
            position: None,
            path: None,
            of_data: true,
        }
    }
}

impl From<serde::de::value::Error> for DecodeError {
    fn from(error: serde::de::value::Error) -> Self {
        Self::from(&error)
    }
}

/// The links of an error's chain that quote a value, each as the text it
/// displays and the sentence it is said as: a decode failure as its report, a
/// link a [`SafeLink`](crate::SafeLink) claims as its driver says it.
pub(crate) struct Redactions(Vec<(String, String)>);

impl Redactions {
    /// Every link of `error`'s chain that says something its safe sentence does
    /// not, down to the first link a driver claims: what is below it is that
    /// driver's to say.
    pub(crate) fn of(error: Option<&(dyn std::error::Error + 'static)>) -> Self {
        let mut pairs = Vec::new();
        let mut link = error;
        while let Some(current) = link {
            let claimed = crate::SafeLink::said(current);
            link = if claimed.is_some() {
                None
            } else {
                current.source()
            };
            let safe =
                claimed.or_else(|| DecodeError::of(current).map(|report| report.to_string()));
            if let Some(said) = safe {
                let displayed = current.to_string();
                if !displayed.is_empty() && displayed != said {
                    pairs.push((displayed, said));
                }
            }
        }
        Self(pairs)
    }

    /// `text`, with each link it spells said as its safe sentence, and any
    /// sentence left in serde's quoting shapes rebuilt without its value.
    pub(crate) fn redact<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let mut text = Cow::Borrowed(text);
        for (displayed, said) in &self.0 {
            if text.contains(displayed.as_str()) {
                text = Cow::Owned(text.replace(displayed.as_str(), said));
            }
        }
        // validator's wording first: a value it lists may spell what serde's
        // shapes look for, and serde's reading keeps what follows them.
        let validated = match validation_sentences(&text) {
            Cow::Owned(validated) => Some(validated),
            Cow::Borrowed(_) => None,
        };
        let text = validated.map_or(text, Cow::Owned);
        let rebuilt = match quoting_sentences(&text) {
            Cow::Owned(rebuilt) => Some(rebuilt),
            Cow::Borrowed(_) => None,
        };
        rebuilt.map_or(text, Cow::Owned)
    }
}

/// `text` within [`DecodeError::MAX_LEN`] bytes, cut at a character boundary,
/// and whether it was cut.
fn within_bound(text: &str) -> (&str, bool) {
    if text.len() <= DecodeError::MAX_LEN {
        return (text, false);
    }
    let mut end = DecodeError::MAX_LEN;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// `text` within [`DecodeError::MAX_LEN`] bytes, ending in `…` when it was cut.
fn bounded(text: &str) -> Cow<'_, str> {
    match within_bound(text) {
        (kept, true) => Cow::Owned(format!("{kept}…")),
        (kept, false) => Cow::Borrowed(kept),
    }
}

/// What a type's refusal in its own words is said as: its message may quote the
/// value it refused.
const REFUSED_BY_ITS_TYPE: &str = "a value its type does not accept";

/// What a data error — a value that is valid JSON and not the type — is reported
/// as, rebuilt from the shapes serde words.
fn data(message: &str) -> String {
    if let Some(rebuilt) = quoting(message) {
        return rebuilt;
    }
    // Sentences naming a count, or a field of the type's own, and nothing the
    // payload holds as a value. `missing field` and `duplicate field` name a
    // field the derive spelled (`&'static str`); `unknown field` names the
    // payload's key, and is a quoting shape above.
    const KEPT: [&str; 5] = [
        "invalid length ",
        "missing field `",
        "duplicate field `",
        "data did not match any variant of untagged enum ",
        "no variant of enum ",
    ];
    if KEPT.iter().any(|kept| message.starts_with(kept)) {
        return message.to_owned();
    }
    REFUSED_BY_ITS_TYPE.to_owned()
}

/// A sentence in one of serde's quoting shapes, rebuilt without what the payload
/// spelled; `None` for any other.
fn quoting(sentence: &str) -> Option<String> {
    if let Some(rest) = sentence.strip_prefix("invalid type: ") {
        return Some(found("invalid type", rest));
    }
    if let Some(rest) = sentence.strip_prefix("invalid value: ") {
        return Some(found("invalid value", rest));
    }
    // The variant and the key are spelled by whoever wrote the payload; the list
    // the type expected is the type's.
    if let Some(rest) = sentence.strip_prefix("unknown variant ") {
        return Some(unknown("unknown variant", rest, ", there are no variants"));
    }
    if let Some(rest) = sentence.strip_prefix("unknown field ") {
        return Some(unknown("unknown field", rest, ", there are no fields"));
    }
    None
}

/// The kinds serde's `Unexpected` quotes a value after, and what each is said as
/// — a string's opening quote also as `Debug` escapes it, which is how a wrapper
/// formatting its cause with `{:?}` spells it.
const QUOTING_KINDS: [(&str, &str); 6] = [
    ("boolean `", "a boolean"),
    ("integer `", "an integer"),
    ("floating point `", "a floating point number"),
    ("character `", "a character"),
    ("string \"", "a string"),
    ("string \\\"", "a string"),
];

/// Where one of serde's quoting sentences opens in `text`, read by serde's own
/// wording: `invalid type:` or `invalid value:` then a kind that quotes a value,
/// or `` unknown variant ` `` / `` unknown field ` ``, each closed later by the
/// tail serde always writes. Anything else — `invalid value: must be positive`,
/// a sentence this module already rebuilt — is somebody's own words, and read as
/// they are.
fn quoting_opens_at(text: &str) -> Option<usize> {
    const FOUND: [&str; 2] = ["invalid type: ", "invalid value: "];
    const UNKNOWN: [&str; 2] = ["unknown variant `", "unknown field `"];
    // Found once: per-opening search is quadratic in a text a client can fill
    // with openings, and an opening is closed exactly when the last tail follows it.
    let expected = text.rfind(", expected ");
    let closed = expected.max(text.rfind(", there are no "));
    let found = FOUND.iter().filter_map(|open| {
        text.match_indices(open)
            .map(|(at, _)| at)
            .take_while(|at| expected.is_some_and(|tail| tail >= at + open.len()))
            .find(|at| {
                let rest = &text[at + open.len()..];
                QUOTING_KINDS.iter().any(|(head, _)| rest.starts_with(head))
            })
    });
    let unknown = UNKNOWN.iter().filter_map(|open| {
        text.find(open)
            .filter(|at| closed.is_some_and(|tail| tail >= *at))
    });
    found.chain(unknown).min()
}

/// `text` with each of serde's quoting sentences in it rebuilt, from the first
/// one to the end of `text` — nothing marks where a failure ends in free text,
/// so all that follows it is the failure's. Bounded before it is read, so the
/// walk over the `expected` tails it recurses into is bounded too.
fn quoting_sentences(text: &str) -> Cow<'_, str> {
    let Some(at) = quoting_opens_at(text) else {
        return Cow::Borrowed(text);
    };
    let (failure, cut) = within_bound(&text[at..]);
    // Every opening `quoting_opens_at` finds is one `quoting` rebuilds; the
    // fallback is the report for a shape it does not know, which says nothing
    // the payload held.
    let rebuilt = quoting(failure).unwrap_or_else(|| REFUSED_BY_ITS_TYPE.to_owned());
    let mut redacted = String::with_capacity(at + rebuilt.len() + '…'.len_utf8());
    redacted.push_str(&text[..at]);
    redacted.push_str(&bounded(&rebuilt));
    if cut && !redacted.ends_with('…') {
        redacted.push('…');
    }
    Cow::Owned(redacted)
}

/// What opens `validator`'s wording of a failing rule: `Validation error: <code>
/// [<params>]`, its `Display` for a rule given no message of its own.
const VALIDATION_OPENING: &str = "Validation error: ";

/// `text` with each `validator` failure in it said without its parameters.
///
/// The parameters hold the rejected input, so they are dropped up to their
/// closing bracket — read past quoted strings, so a value cannot close them
/// early — or to the end of `text`, marked `…`. The rule's code is kept. Each
/// stretch of `text` is read a bounded number of times.
fn validation_sentences(text: &str) -> Cow<'_, str> {
    if !text.contains(VALIDATION_OPENING) {
        return Cow::Borrowed(text);
    }
    let mut said = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(VALIDATION_OPENING) {
        let after = &rest[at + VALIDATION_OPENING.len()..];
        // The code ends at ` [`, before the next opening and on the same line.
        let next = after.find(VALIDATION_OPENING).unwrap_or(after.len());
        let line = after[..next].split('\n').next().unwrap_or_default();
        let Some(open) = line.find(" [") else {
            said.push_str(&rest[..at + VALIDATION_OPENING.len()]);
            rest = after;
            continue;
        };
        said.push_str(&rest[..at + VALIDATION_OPENING.len() + open]);
        match past_closing_bracket(&after[open + 2..]) {
            Some(tail) => rest = tail,
            None => {
                said.push('…');
                rest = "";
            }
        }
    }
    said.push_str(rest);
    Cow::Owned(said)
}

/// What follows the `]` closing a bracket already open, read past quoted
/// strings and their escapes; `None` when it never closes.
fn past_closing_bracket(text: &str) -> Option<&str> {
    let mut depth = 1_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if quoted {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[at + 1..]);
                }
            }
            _ => {}
        }
    }
    None
}

/// `invalid type` or `invalid value`, said with the kind of value found and the
/// type expected.
fn found(what: &str, rest: &str) -> String {
    let kind = kind(rest);
    match expected(rest) {
        Some(expected) => format!("{what}: {kind}, {}", quoting_sentences(expected)),
        None => format!("{what}: {kind}"),
    }
}

/// `unknown variant` or `unknown field`, said with what the type expected and
/// without what the payload spelled.
fn unknown(what: &str, rest: &str, none_expected: &str) -> String {
    match expected(rest) {
        Some(expected) => format!("{what}, {}", quoting_sentences(expected)),
        None if rest.ends_with(none_expected) => format!("{what}{none_expected}"),
        None => what.to_owned(),
    }
}

/// The `expected …` tail serde closes a refusal with. The last one: a value
/// spelled before it may itself contain the words, the type's description after
/// it does not.
fn expected(rest: &str) -> Option<&str> {
    rest.rfind(", expected ").map(|at| &rest[at + 2..])
}

/// The kind of value serde's `Unexpected` names at the head of `rest`, without
/// the value it quotes after the kind.
fn kind(rest: &str) -> &'static str {
    // The kinds that quote nothing.
    const BARE: [(&str, &str); 12] = [
        ("unit value", "null"),
        ("byte array", "a byte array"),
        ("Option value", "an optional value"),
        ("newtype struct", "a newtype struct"),
        ("sequence", "a sequence"),
        ("map", "a map"),
        ("enum", "an enum"),
        ("unit variant", "a unit variant"),
        ("newtype variant", "a newtype variant"),
        ("tuple variant", "a tuple variant"),
        ("struct variant", "a struct variant"),
        ("null", "null"),
    ];
    if let Some((_, kind)) = QUOTING_KINDS
        .iter()
        .find(|(head, _)| rest.starts_with(head))
    {
        return kind;
    }
    BARE.iter()
        .find(|(head, _)| {
            rest.strip_prefix(head)
                .is_some_and(|tail| tail.is_empty() || tail.starts_with(", expected "))
        })
        .map_or("a value", |(_, kind)| kind)
}

#[cfg(test)]
mod decode_error_tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize)]
    enum Card {
        Visa,
    }

    #[derive(Debug, Deserialize)]
    #[expect(
        dead_code,
        reason = "the fields exist for serde to read; the test asserts the error, never a value"
    )]
    struct Charge {
        amount: u64,
        card: Card,
    }

    fn from_value(value: serde_json::Value) -> String {
        let error = serde_json::from_value::<Charge>(value).expect_err("does not decode");
        DecodeError::new(&error).to_string()
    }

    fn from_str(text: &str) -> String {
        let error = serde_json::from_str::<Charge>(text).expect_err("does not decode");
        DecodeError::new(&error).to_string()
    }

    /// The CPU time this thread has run. A thread waiting for a core accrues
    /// none, so a cost read on it holds however loaded the machine is.
    #[expect(unsafe_code, reason = "std has no thread CPU clock")]
    fn thread_cpu_time() -> std::time::Duration {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `now` is a valid `timespec` the call only writes.
        let read = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
        assert_eq!(read, 0, "the thread's CPU clock reads");
        std::time::Duration::new(
            u64::try_from(now.tv_sec).expect("a CPU time is not negative"),
            u32::try_from(now.tv_nsec).expect("nanoseconds below a second"),
        )
    }

    #[test]
    fn a_refused_value_is_said_by_its_kind_and_never_quoted() {
        let secret = from_value(json!({ "amount": "sk_live_51HsecretTOKEN", "card": "Visa" }));
        assert_eq!(secret, "invalid type: a string, expected u64");
        let card = from_value(json!({ "amount": 1, "card": "4242424242424242" }));
        assert_eq!(card, "unknown variant, expected `Visa`");
        for report in [secret, card] {
            assert!(
                !report.contains("sk_live") && !report.contains("4242"),
                "{report}"
            );
        }
    }

    #[test]
    fn a_payload_read_from_text_keeps_its_line_and_column() {
        assert_eq!(
            from_str(r#"{"amount": true, "card": "Visa"}"#),
            "invalid type: a boolean, expected u64 at line 1 column 15"
        );
        assert_eq!(
            from_str(r#"{"amount": 1, "card": "Visa""#),
            "EOF while parsing an object at line 1 column 28",
            "a fault of the text is serde_json's own sentence, which quotes nothing",
        );
    }

    /// A value spelling serde's own separator cannot move the cut into the type's
    /// half of the sentence.
    #[test]
    fn a_value_spelling_the_separator_is_still_not_quoted() {
        let report = from_value(json!({ "amount": "x, expected u64, leaked", "card": "Visa" }));
        assert_eq!(report, "invalid type: a string, expected u64");
    }

    #[test]
    fn kinds_and_the_type_s_own_sentences_are_kept() {
        assert_eq!(
            from_value(json!({ "amount": null, "card": "Visa" })),
            "invalid type: null, expected u64"
        );
        assert_eq!(
            from_value(json!({ "amount": [1], "card": "Visa" })),
            "invalid type: a sequence, expected u64"
        );
        assert_eq!(
            from_value(json!({ "amount": -5, "card": "Visa" })),
            "invalid value: an integer, expected u64"
        );
        assert_eq!(
            from_value(json!({ "amount": 1.5, "card": "Visa" })),
            "invalid type: a floating point number, expected u64"
        );
        assert_eq!(
            from_value(json!({ "card": "Visa" })),
            "missing field `amount`"
        );
    }

    /// `unknown field` names the key the *payload* spelled, so it is dropped like
    /// a variant: what is kept is the list the type expected.
    #[test]
    fn an_unknown_key_is_the_payload_s_and_is_dropped() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts the error, never a value"
        )]
        struct Strict {
            amount: u64,
        }
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts the error, never a value"
        )]
        struct Pair {
            amount: u64,
            currency: String,
        }
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Empty {}

        let text = serde_json::from_str::<Strict>(r#"{"amount":1,"sk_live_KEY":1}"#)
            .expect_err("an unknown key");
        assert_eq!(
            DecodeError::new(&text).to_string(),
            "unknown field, expected `amount` at line 1 column 25"
        );
        let value = serde_json::from_value::<Strict>(json!({ "amount": 1, "sk_live_KEY": 1 }))
            .expect_err("an unknown key");
        assert_eq!(
            DecodeError::new(&value).to_string(),
            "unknown field, expected `amount`"
        );
        let pair = serde_json::from_value::<Pair>(json!({ "sk_live_KEY": 1 }))
            .expect_err("an unknown key");
        assert_eq!(
            DecodeError::new(&pair).to_string(),
            "unknown field, expected `amount` or `currency`"
        );
        let empty =
            serde_json::from_value::<Empty>(json!({ "sk_live_KEY": 1 })).expect_err("no fields");
        assert_eq!(
            DecodeError::new(&empty).to_string(),
            "unknown field, there are no fields"
        );
    }

    #[test]
    fn what_a_report_keeps_is_bounded() {
        #[derive(Debug)]
        struct Verbose;
        impl<'de> Deserialize<'de> for Verbose {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct Visitor;
                impl serde::de::Visitor<'_> for Visitor {
                    type Value = Verbose;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(&"é".repeat(2 * DecodeError::MAX_LEN))
                    }
                }
                d.deserialize_u64(Visitor)
            }
        }
        let error = serde_json::from_str::<Verbose>("true").expect_err("not a number");
        let report = DecodeError::new(&error).to_string();
        let (sentence, position) = report
            .rsplit_once('…')
            .expect("a report past the bound says it was cut");
        assert!(sentence.len() <= DecodeError::MAX_LEN, "{}", sentence.len());
        assert!(sentence.starts_with("invalid type: a boolean, expected é"));
        assert_eq!(position, " at line 1 column 4");
    }

    #[test]
    fn serde_s_value_error_is_reported_like_serde_json_s() {
        use serde::de::IntoDeserializer;
        use serde::de::value::Error as ValueError;

        let quoted = u64::deserialize(IntoDeserializer::<ValueError>::into_deserializer(
            "sk_live_51HsecretTOKEN",
        ))
        .expect_err("not a number");
        assert!(
            quoted.to_string().contains("sk_live"),
            "serde quotes it: {quoted}"
        );
        assert_eq!(
            DecodeError::from(&quoted).to_string(),
            "invalid type: a string, expected u64"
        );
        let custom = <ValueError as serde::de::Error>::custom("token ya29.secret refused");
        assert_eq!(
            DecodeError::from(custom).to_string(),
            "a value its type does not accept"
        );
    }

    #[test]
    fn a_rendered_text_is_redacted_by_the_chain_and_by_serde_s_wording() {
        let secret =
            serde_json::from_str::<u64>(r#""sk_live_51HsecretTOKEN""#).expect_err("not a number");
        let detail = format!("parse error: {secret}");
        assert_eq!(
            DecodeError::redact(&detail, Some(&secret)),
            "parse error: invalid type: a string, expected u64 at line 1 column 24"
        );
        assert_eq!(
            DecodeError::redact(&detail, None),
            "parse error: invalid type: a string, expected u64 at line 1 column 24",
            "serde's wording is read without the chain too",
        );
        assert!(
            matches!(
                DecodeError::redact("not found", Some(&secret)),
                Cow::Borrowed(_)
            ),
            "a text quoting nothing is handed back as it was",
        );

        // A type's `custom` message has no shape to read; only the chain knows it.
        let custom = <serde_json::Error as serde::de::Error>::custom("bad token sk_live_51H");
        let detail = format!("could not read: {custom}");
        assert_eq!(
            DecodeError::redact(&detail, Some(&custom)),
            "could not read: a value its type does not accept"
        );

        // The quoted value may spell anything, the separator and a second
        // sentence included; every shape after the first is rebuilt as well.
        let text = r#"x: invalid type: string "a, expected u64, b", expected u64; unknown field `sk_live`, there are no fields"#;
        let redacted = DecodeError::redact(text, None);
        assert!(
            !redacted.contains("sk_live") && !redacted.contains("\"a"),
            "{redacted}"
        );
        assert_eq!(
            redacted,
            "x: invalid type: a string, expected u64; unknown field, there are no fields"
        );

        // Somebody's own words in serde's vocabulary are not serde's sentence.
        for own in [
            "invalid value: must be positive",
            "invalid type: expected an admin token",
            "unknown field `limit` in the request",
        ] {
            assert_eq!(DecodeError::redact(own, None), own);
        }

        // A report read again reads the same, so the two readings compose.
        for report in [
            "invalid type: a string, expected u64 at line 1 column 24",
            "invalid value: an integer, expected u64",
            "unknown variant, expected `Visa`",
            "unknown field, there are no fields",
        ] {
            assert_eq!(DecodeError::redact(report, None), report);
        }

        // A client can fill a text with openings; reading them is linear.
        let crowded = format!(
            "{}, expected u64",
            "invalid type: string \"".repeat(200_000)
        );
        let started = thread_cpu_time();
        let redacted = DecodeError::redact(&crowded, None);
        let spent = thread_cpu_time() - started;
        assert!(
            redacted.len() < 2 * DecodeError::MAX_LEN,
            "{}",
            redacted.len()
        );
        assert!(
            spent < std::time::Duration::from_secs(1),
            "{spent:?} of this thread's CPU for {} bytes",
            crowded.len(),
        );

        // Nothing marks where a failure ends in free text, so what follows the
        // first one is the failure's, and bounded like a report.
        let long = format!(
            "x: invalid type: string \"{}\", expected u64",
            "s".repeat(10_000)
        );
        let redacted = DecodeError::redact(&long, None);
        assert!(
            redacted.len() < 2 * DecodeError::MAX_LEN,
            "{}",
            redacted.len()
        );
        assert!(!redacted.contains("sss"), "{redacted}");
    }

    #[test]
    fn a_message_of_no_known_shape_is_reported_without_its_text() {
        let error = <serde_json::Error as serde::de::Error>::custom("token ya29.secret refused");
        assert_eq!(
            DecodeError::new(&error).to_string(),
            "a value its type does not accept"
        );
    }

    #[test]
    fn a_validator_failure_keeps_its_rule_and_drops_its_parameters() {
        let text = "signup refused: password: Validation error: length \
                    [{\"min\": Number(32), \"value\": String(\"sk_live_secret\")}]\n\
                    email: Validation error: email [{\"value\": String(\"sk_live_secret\")}]";
        assert_eq!(
            DecodeError::redact(text, None),
            "signup refused: password: Validation error: length\nemail: Validation error: email"
        );
    }

    /// A value spelling the closing bracket, or a quote, cannot end the
    /// parameters early; parameters that never close run to the end of the text.
    #[test]
    fn a_validator_parameter_cannot_close_itself_early() {
        let spelled =
            r#"x: Validation error: must_match [{"other": String("a]\"b] sk_live_secret")}] tail"#;
        assert_eq!(
            DecodeError::redact(spelled, None),
            "x: Validation error: must_match tail"
        );
        let cut = r#"x: Validation error: length [{"value": String("sk_live_sec"#;
        assert_eq!(
            DecodeError::redact(cut, None),
            "x: Validation error: length…"
        );
    }

    /// serde's reading keeps the text after the last `, expected `, which a value
    /// validator lists can spell; validator's wording is read first.
    #[test]
    fn a_validator_value_cannot_hide_behind_a_serde_sentence() {
        let text = r#"body: invalid type: string "x", expected u64; pw: Validation error: length [{"value": String("zz, expected sk_live_secret")}]"#;
        let redacted = DecodeError::redact(text, None);
        assert!(!redacted.contains("sk_live"), "{redacted}");
    }

    #[test]
    fn openings_without_parameters_are_left_as_they_are() {
        let crowded = "Validation error: x ".repeat(10_000);
        assert_eq!(DecodeError::redact(&crowded, None), crowded);
    }

    #[test]
    fn a_validator_rule_with_its_own_message_is_left_to_its_author() {
        let text = "password: must be at least 32 characters";
        assert!(matches!(DecodeError::redact(text, None), Cow::Borrowed(_)));
    }

    /// The report of `text` failing to decode as `T`, located.
    fn located<T: for<'de> Deserialize<'de> + std::fmt::Debug>(text: &str) -> DecodeError {
        let error = serde_json::from_str::<T>(text).expect_err("does not decode");
        DecodeError::new(&error).located::<T, _>(&mut serde_json::Deserializer::from_str(text))
    }

    #[test]
    fn a_located_report_says_where_and_drops_the_position_the_path_replaces() {
        let report = located::<Charge>(r#"{"amount":"sk_live_51","card":"Visa"}"#);
        assert_eq!(report.path(), Some("amount"));
        assert_eq!(report.to_string(), "invalid type: a string, expected u64");

        let report = located::<Charge>(r#"{"amount":1,"card":"sk_live_51"}"#);
        assert_eq!(report.path(), Some("card"));
        assert_eq!(report.to_string(), "unknown variant, expected `Visa`");
    }

    #[test]
    fn a_field_missing_or_given_twice_is_placed_at_the_field() {
        let report = located::<Charge>(r#"{"amount":1}"#);
        assert_eq!(report.path(), Some("card"));
        assert_eq!(report.to_string(), "missing");

        let report = located::<Charge>(r#"{"amount":1,"amount":2,"card":"Visa"}"#);
        assert_eq!(report.path(), Some("amount"));
        assert_eq!(report.to_string(), "duplicate");
    }

    #[test]
    fn a_type_s_own_refusal_says_what_it_expected_and_never_the_value() {
        #[derive(Debug)]
        struct Even;
        impl<'de> Deserialize<'de> for Even {
            fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                struct Visit;
                impl serde::de::Visitor<'_> for Visit {
                    type Value = Even;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str("an even number")
                    }
                    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Even, E> {
                        Err(E::custom(format!("{value} is odd")))
                    }
                }
                de.deserialize_u64(Visit)
            }
        }
        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the field exists for serde to read; the test asserts the error, never a value"
        )]
        struct Lucky {
            lucky: Even,
        }
        let report = located::<Lucky>(r#"{"lucky":51}"#);
        assert_eq!(report.path(), Some("lucky"));
        assert_eq!(
            report.to_string(),
            "a value its type does not accept, expected an even number"
        );
    }

    #[test]
    fn what_a_type_expected_is_bounded_with_the_sentence_it_joins() {
        #[derive(Debug)]
        struct Verbose;
        impl<'de> Deserialize<'de> for Verbose {
            fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                struct Visit;
                impl serde::de::Visitor<'_> for Visit {
                    type Value = Verbose;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(&"é".repeat(DecodeError::MAX_LEN))
                    }
                    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Verbose, E> {
                        Err(E::custom(format!("{value} is refused")))
                    }
                }
                de.deserialize_u64(Visit)
            }
        }
        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the field exists for serde to read; the test asserts the error, never a value"
        )]
        struct Holder {
            verbose: Verbose,
        }
        let report = located::<Holder>(r#"{"verbose":51}"#).to_string();
        let sentence = report
            .strip_suffix('…')
            .expect("a report past the bound says it was cut");
        assert!(sentence.len() <= DecodeError::MAX_LEN, "{}", sentence.len());
        assert!(
            sentence.starts_with("a value its type does not accept, expected é"),
            "{sentence}"
        );
    }

    #[test]
    fn a_report_that_is_not_a_type_s_keeps_its_line_and_column() {
        let report = located::<Charge>(r#"{"amount":"#);
        assert_eq!(report.path(), None);
        assert_eq!(
            report.to_string(),
            "EOF while parsing a value at line 1 column 10"
        );
    }

    #[test]
    fn a_failure_of_the_payload_as_a_whole_has_no_path() {
        let report = located::<Charge>(r#""sk_live_51""#);
        assert_eq!(report.path(), None);
        assert_eq!(
            report.to_string(),
            "invalid type: a string, expected struct Charge"
        );
    }

    #[test]
    fn serde_s_own_value_error_is_located_the_same_way() {
        use serde::de::value::{Error, MapDeserializer};

        let pairs = || {
            MapDeserializer::<_, Error>::new(
                [("amount", "sk_live_51"), ("card", "Visa")].into_iter(),
            )
        };
        let refused = Charge::deserialize(pairs()).expect_err("amount is no number");
        let report = DecodeError::from(&refused).located::<Charge, _>(pairs());
        assert_eq!(report.path(), Some("amount"));
        assert_eq!(report.to_string(), "invalid type: a string, expected u64");
    }
}

#[cfg(test)]
mod boot_refusal_tests {
    use super::*;

    #[test]
    fn a_provider_cycle_names_its_chain_and_prescribes_an_edit_that_works() {
        let cycle = ProviderCycleError {
            module: "PostsModule",
            scope: ProviderScope::Request,
            chain: vec!["Drafts", "Reviews", "Drafts"],
        };
        assert_eq!(
            cycle.to_string(),
            "module `PostsModule`: request-scoped provider cycle `Drafts` → `Reviews` → `Drafts` \
             — break it by moving what both need into a third provider both inject, by passing \
             the value as a method argument, or by publishing an event one side listens to"
        );
        let sentences = [
            cycle.to_string(),
            ProviderCycleError {
                scope: ProviderScope::Singleton,
                ..cycle
            }
            .to_string(),
            cycle_panic_message(ProviderScope::Transient, "A → B → A"),
            cycle_panic_message(ProviderScope::Request, "A → B → A"),
        ];
        for sentence in sentences {
            assert!(
                !sentence.contains("dyn"),
                "an `Arc<dyn Trait>` is a dependency gated like a concrete one, so injecting one \
                 keeps the cycle: {sentence}"
            );
            assert!(sentence.ends_with(CYCLE_REMEDY), "{sentence}");
        }
    }

    #[test]
    fn a_provider_never_built_names_what_it_needed_and_the_edit_its_cause_has() {
        let cycle = UnbuiltProviderError {
            module: "UsersModule",
            provider: "UsersService",
            missing: vec!["PostsService"],
            cause: UnbuiltCause::ImportCycle(vec!["UsersModule", "PostsModule"]),
        };
        assert_eq!(
            cycle.to_string(),
            "module `UsersModule`: `UsersService` was never built — it needs `PostsService`, \
             which nothing had registered when `UsersModule`'s providers were built; \
             `UsersModule` and `PostsModule` import each other, so one registers before the \
             other's providers exist: move what both need into a module both import"
        );
        let longer = UnbuiltProviderError {
            cause: UnbuiltCause::ImportCycle(vec!["UsersModule", "PostsModule", "OrgsModule"]),
            ..cycle
        };
        assert!(
            longer.to_string().contains(
                "`UsersModule`, `PostsModule` and `OrgsModule` import each other in a cycle, so \
                 one registers before another's providers exist: move what they need into a \
                 module they all import"
            ),
            "{longer}"
        );
        let later = UnbuiltProviderError {
            module: "BillingModule",
            provider: "Invoices",
            missing: vec!["Ledger", "Clock"],
            cause: UnbuiltCause::RegisteredLater,
        };
        assert_eq!(
            later.to_string(),
            "module `BillingModule`: `Invoices` was never built — it needs `Ledger` and `Clock`, \
             which nothing had registered when `BillingModule`'s providers were built; a \
             hand-written module or a `for_root` outside `BillingModule`'s imports registers \
             them later: import that module in `BillingModule`'s `imports`"
        );
    }

    #[test]
    fn an_interruption_names_its_cause_and_its_step() {
        let factory = BootInterruptedError {
            cause: crate::StopCause::Terminate,
            step: crate::BootStep::Factory {
                type_name: "app::Pool",
            },
        };
        assert_eq!(
            factory.to_string(),
            "the boot was stopped by SIGTERM during the factory for `Pool`"
        );
        let configure = BootInterruptedError {
            cause: crate::StopCause::Interrupt,
            step: crate::BootStep::Configure {
                transport: "http".to_owned(),
            },
        };
        assert_eq!(
            configure.to_string(),
            "the boot was stopped by SIGINT during the `configure` of transport `http`"
        );
        let hook = BootInterruptedError {
            cause: crate::StopCause::Terminate,
            step: crate::BootStep::InitHook {
                phase: crate::LifecyclePhase::OnModuleInit,
                provider: "CacheWarmer",
                method: "warm",
                origin: "app::cache",
            },
        };
        assert_eq!(
            hook.to_string(),
            "the boot was stopped by SIGTERM during the `#[on_module_init]` hook \
             `CacheWarmer::warm` in `app::cache`"
        );
        let closed = BootInterruptedError {
            cause: crate::StopCause::Closed,
            step: crate::BootStep::Read {
                type_name: "nest_rs_http::config::HttpConfig",
            },
        };
        assert_eq!(
            closed.to_string(),
            "the boot was stopped by `App::close` during the read of `HttpConfig`"
        );
    }

    #[test]
    fn a_failed_factory_is_named_and_says_its_cause_after_it() {
        let failed = FactoryFailedError {
            type_name: "app::Pool",
            source: anyhow::anyhow!("connection refused"),
        };
        assert_eq!(
            crate::error_message(&failed),
            "the factory for `Pool` failed: connection refused"
        );
    }

    #[test]
    fn a_boot_refusal_names_a_type_by_the_path_an_app_writes() {
        let duplicate = DuplicateProviderError {
            type_name: "api::users::service::UsersService",
            first: "`UsersModule`".to_owned(),
            second: "`AdminModule`".to_owned(),
            scopes: None,
        };
        assert!(
            duplicate
                .to_string()
                .starts_with("duplicate provider: `UsersService` is registered by"),
            "{duplicate}"
        );
        let cycle = FactoryCycleError {
            type_names: vec!["app::A", "app::B<app::C>"],
        };
        assert!(
            cycle
                .to_string()
                .starts_with("factory cycle: `A` and `B<C>` — "),
            "{cycle}"
        );
    }
}
