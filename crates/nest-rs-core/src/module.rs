//! The [`DynamicModule`] trait — the contract a configured import
//! (`Foo::for_root(opts)`) implements to seed values or queue async factories
//! at its import site, distinct from a bare `#[module]` type.

use crate::container::ContainerBuilder;

/// Boot-time trace emitted by the `#[module]` macro after a module finishes
/// registering its providers. Idempotent registration means a diamond import
/// fires this exactly once. Target `nest_rs::module`, level `info` — quiet under
/// `RUST_LOG=warn`, visible by default.
#[doc(hidden)]
pub fn __module_registered(name: &'static str) {
    tracing::info!(
        target: crate::target::MODULE,
        module = name,
        "module dependencies initialized",
    );
}

/// The type of a dynamic import's value, read off its expression without
/// evaluating it, so the access graph can name the import before the boot runs.
///
/// **Internal ABI** — emitted by `#[module]`, lockstep with
/// `nest-rs-core-macros`; do not call by hand.
#[doc(hidden)]
pub fn __dynamic_import_type<D: 'static>(_import: impl FnOnce() -> D) -> std::any::TypeId {
    std::any::TypeId::of::<D>()
}

/// A statically-composed module — the common case, listed by type in
/// `#[module(imports = [...])]`. The `#[module]` macro makes registration
/// idempotent via [`ContainerBuilder::mark_registered`], so a diamond import
/// builds its providers exactly once.
pub trait Module {
    /// Build this module's providers and recurse into imports. Runs in the
    /// register phase, after every async factory has produced its value.
    fn register(builder: ContainerBuilder) -> ContainerBuilder;

    /// Queue the async factories declared by this module's import tree.
    /// Default is a no-op; the `#[module]` macro overrides it to recurse.
    fn collect(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}

/// A module configured at its import site (e.g. `Module::for_root(opts)`),
/// built synchronously via `register` or asynchronously via `collect`.
///
/// Unlike [`Module`], a dynamic module is a value that captures options:
///
/// ```
/// # use nest_rs_core::{App, ContainerBuilder, DynamicModule, module};
/// # #[module]
/// # pub struct UsersModule;
/// # pub struct Greeting(&'static str);
/// # pub struct GreetingModule;
/// # pub struct GreetingSetup(Greeting);
/// # impl GreetingModule {
/// #     pub fn for_root(greeting: Greeting) -> GreetingSetup { GreetingSetup(greeting) }
/// # }
/// # impl DynamicModule for GreetingSetup {
/// #     fn register(self, builder: ContainerBuilder) -> ContainerBuilder { builder.provide(self.0) }
/// # }
/// #[module(imports = [
///     UsersModule,                                  // static, by type
///     GreetingModule::for_root(Greeting("hello")),  // dynamic, configured at its import site
/// ])]
/// pub struct AppModule;
///
/// let app = App::new::<AppModule>()?;
/// assert_eq!(app.container().get::<Greeting>().map(|g| g.0), Some("hello"));
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// Dynamic modules are **not** auto-deduplicated — each carries its own
/// config.
///
/// # Importing it imports the module it registers
///
/// A dynamic import whose [`register`] registers a module *is* that module,
/// configured, as NestJS's `DynamicModule { module }` is: a provider beside the
/// import injects what the module provides, as beside a static import.
///
/// ```
/// # use std::sync::Arc;
/// # use nest_rs_core::{App, ContainerBuilder, DynamicModule, Module, injectable, module};
/// #[injectable]
/// pub struct Client;
///
/// #[module(providers = [Client])]
/// pub struct ClientModule;
///
/// pub struct ClientSetup;
///
/// impl DynamicModule for ClientSetup {
///     fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
///         <ClientModule as Module>::register(builder)
///     }
/// }
/// # impl ClientModule {
/// #     pub fn for_root() -> ClientSetup { ClientSetup }
/// # }
///
/// #[injectable]
/// pub struct Uploads {
///     #[inject]
///     client: Arc<Client>,
/// }
///
/// #[module(imports = [ClientModule::for_root()], providers = [Uploads])]
/// pub struct UploadsModule;
///
/// let app = App::new::<UploadsModule>()?;
/// assert!(app.container().get::<Uploads>().is_some());
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// Two phases, both defaulting to no-op:
///
/// - [`collect`](Self::collect) — queue an async factory (for resources like
///   a DB pool that must be built asynchronously).
/// - [`register`](Self::register) — install synchronous providers, metadata,
///   or config.
///
/// # The import expression is evaluated exactly once
///
/// `#[module(imports = [Foo::for_root(opts)])]` builds the value in the
/// [`collect`] phase and parks it on the [`ContainerBuilder`], so [`register`]
/// consumes *that* value rather than re-running the expression; the
/// synchronous [`App::new`](crate::App::new) path has no collect phase and
/// builds it in `register` instead. Both phases therefore see the same value,
/// and a `for_root` that is not idempotent still behaves (it runs once).
///
/// Because the value outlives its construction site, an implementor must be
/// `Send + 'static` to be usable from `#[module(imports = [...])]`.
///
/// [`collect`]: Self::collect
/// [`register`]: Self::register
pub trait DynamicModule {
    /// Install synchronous providers, metadata or config from this module's
    /// configuration. Consumes `self` — the config is moved into the providers.
    /// Defaults to a no-op for modules that only queue async work in
    /// [`collect`](Self::collect).
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder
    where
        Self: Sized,
    {
        builder
    }

    /// Queue an async factory (for resources like a DB pool that must be built
    /// asynchronously) to be awaited in the factories phase. Takes `&self`, and
    /// the very same value is handed to [`register`](Self::register) afterwards
    /// (see the trait docs). Defaults to a no-op.
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}
