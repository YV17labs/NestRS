//! The [`DynamicModule`] trait — the contract a configured import
//! (`Foo::for_root(opts)`) implements to seed values or queue async factories
//! at its import site, distinct from a bare `#[module]` type.

use std::any::TypeId;

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

/// The module a dynamic import declares it is, read off its expression's type
/// without evaluating it, so the access graph has the import before the boot
/// runs.
///
/// **Internal ABI** — emitted by `#[module]`, lockstep with
/// `nest-rs-core-macros`; do not call by hand.
#[doc(hidden)]
pub fn __dynamic_import_module<D: DynamicModule>(_import: impl FnOnce() -> D) -> TypeId {
    D::module()
}

/// A statically-composed module — the common case, listed by type in
/// `#[module(imports = [...])]`. The `#[module]` macro makes registration
/// idempotent via [`ContainerBuilder::mark_registered`], so a diamond import
/// builds its providers exactly once.
///
/// # Written by hand
///
/// An importer runs [`collect`](Self::collect), then [`register`](Self::register),
/// as `#[module]` and the app builder do. A hand-written module whose `collect`
/// queues anything dedupes it with [`ContainerBuilder::mark_collected`] and
/// starts its `register` with `Self::collect(builder)`, as the macro's
/// expansion does: registered by an importer that skipped `collect`, it then
/// has what it queues refused by name
/// ([`LateFactoryError`](crate::LateFactoryError)) rather than never built.
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
/// # use std::any::TypeId;
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
/// #     fn module() -> TypeId { TypeId::of::<GreetingModule>() }
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
/// # Importing it imports the module it declares
///
/// A dynamic import *is* the module its [`module`] names, configured, as
/// NestJS's `DynamicModule { module }` is: a provider beside the import injects
/// what that module provides, as beside a static import, whatever [`register`]
/// wires first. A façade that is no `#[module]` names itself, and its import
/// contributes global infrastructure only.
///
/// ```
/// # use std::any::TypeId;
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
///     fn module() -> TypeId {
///         TypeId::of::<ClientModule>()
///     }
///
///     fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
///         <ClientModule as Module>::collect(builder)
///     }
///
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
/// Beside [`module`], two phases, both defaulting to no-op:
///
/// - [`collect`](Self::collect) — queue an async factory (for resources like
///   a DB pool that must be built asynchronously).
/// - [`register`](Self::register) — install synchronous providers, metadata,
///   or config.
///
/// A setup that wires the module it declares recurses into it in **both**: the
/// module's own `collect` queues what its imports open. Left out, a `#[module]`
/// collects in `register` instead, too late for a factory, and the boot fails
/// with [`LateFactoryError`](crate::LateFactoryError) naming what it would have
/// built.
///
/// # The import expression is evaluated exactly once
///
/// `#[module(imports = [Foo::for_root(opts)])]` builds the value in the
/// [`collect`] phase and parks it on the [`ContainerBuilder`], so [`register`]
/// consumes *that* value rather than re-running the expression — a `#[module]`
/// registered before any collect phase collects itself first. Both phases
/// therefore see the same value, and a `for_root` that is not idempotent still
/// behaves (it runs once).
///
/// Because the value outlives its construction site, an implementor must be
/// `Send + 'static` to be usable from `#[module(imports = [...])]`.
///
/// [`collect`]: Self::collect
/// [`module`]: Self::module
/// [`register`]: Self::register
pub trait DynamicModule {
    /// The module this import *is*: the access graph reads the import as an
    /// import of that module, so a provider beside it may inject what it
    /// provides. Required, as NestJS's `DynamicModule { module }` is; a façade
    /// that is no `#[module]` names itself.
    fn module() -> TypeId
    where
        Self: Sized;

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
