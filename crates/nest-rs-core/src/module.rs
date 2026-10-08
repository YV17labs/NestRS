//! [`Module`] and [`DynamicModule`]: the contracts a module listed by type and
//! one configured at its import site (`Foo::for_root(opts)`) implement.

use std::any::TypeId;
use std::marker::PhantomData;

use crate::container::ContainerBuilder;

/// The framework's proof that `M`'s collect phase runs through an import, once
/// per app; only the framework makes one (`.claude/decisions/module-phases-by-import.md`).
pub struct Collecting<M: ?Sized>(PhantomData<fn(&M)>);

impl<M: ?Sized> Collecting<M> {
    pub(crate) fn new() -> Self {
        Self(PhantomData)
    }
}

/// [`Collecting`]'s twin for `M`'s register phase, run once per app after its collect.
pub struct Registering<M: ?Sized>(PhantomData<fn(&M)>);

impl<M: ?Sized> Registering<M> {
    pub(crate) fn new() -> Self {
        Self(PhantomData)
    }
}

/// Logs that the module `name` registered its providers; emitted by `#[module]`.
#[doc(hidden)]
pub fn __module_registered(name: &'static str) {
    tracing::info!(
        target: crate::target::MODULE,
        module = name,
        "module dependencies initialized",
    );
}

/// The module a dynamic import declares it is, read off its expression's type
/// without evaluating it.
///
/// **Internal ABI** — emitted by `#[module]`, lockstep with
/// `nest-rs-core-macros`; do not call by hand.
#[doc(hidden)]
pub fn __dynamic_import_module<D: DynamicModule>(_import: impl FnOnce() -> D) -> TypeId {
    D::module()
}

/// A module listed by type in `#[module(imports = [...])]`; each phase runs
/// once per app, so a diamond import builds its providers once.
///
/// # Written by hand
///
/// A hand-written module imports its modules with [`ContainerBuilder::import`]
/// in both phases: `register` alone fails the boot with
/// [`LateFactoryError`](crate::LateFactoryError), `collect` alone with
/// [`UnregisteredModuleError`](crate::UnregisteredModuleError).
pub trait Module: 'static {
    /// Build this module's providers, after every async factory has produced its value.
    fn register(builder: ContainerBuilder, registering: Registering<Self>) -> ContainerBuilder;

    /// Queue the async factories this module and its imports declare.
    fn collect(builder: ContainerBuilder, collecting: Collecting<Self>) -> ContainerBuilder {
        let _ = collecting;
        builder
    }
}

/// A module configured at its import site (e.g. `Module::for_root(opts)`): a
/// value that captures options.
///
/// ```
/// # use std::any::TypeId;
/// # use nest_rs_core::{App, ContainerBuilder, DynamicModule, Registering, module};
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
/// #     fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
/// #         builder.provide(self.0)
/// #     }
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
/// Dynamic modules are **not** deduplicated — each carries its own config.
///
/// # Importing it imports the module it declares
///
/// A provider beside the import injects what the module its [`module`] names
/// provides; a façade that is no `#[module]` names itself and contributes
/// global infrastructure only.
///
/// ```
/// # use std::any::TypeId;
/// # use std::sync::Arc;
/// # use nest_rs_core::{App, Collecting, ContainerBuilder, DynamicModule, Registering, injectable, module};
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
///     fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
///         builder.import::<ClientModule>()
///     }
///
///     fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
///         builder.import::<ClientModule>()
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
/// A setup that wires the module it declares imports it in **both** phases
/// ([`ContainerBuilder::import`]); in `register` alone the boot fails with
/// [`LateFactoryError`](crate::LateFactoryError).
///
/// # The import expression is evaluated exactly once
///
/// [`collect`] and [`register`] see the same value, so an implementor must be
/// `Send + 'static` to be usable from `#[module(imports = [...])]`.
///
/// [`collect`]: Self::collect
/// [`module`]: Self::module
/// [`register`]: Self::register
pub trait DynamicModule {
    /// The module this import *is*, as NestJS's `DynamicModule { module }`; a
    /// façade that is no `#[module]` names itself.
    fn module() -> TypeId
    where
        Self: Sized;

    /// Install synchronous providers, metadata or config from this module's
    /// configuration.
    fn register(self, builder: ContainerBuilder, registering: Registering<Self>) -> ContainerBuilder
    where
        Self: Sized,
    {
        let _ = registering;
        builder
    }

    /// Queue an async factory (a DB pool, say) to be awaited in the factories
    /// phase.
    fn collect(&self, builder: ContainerBuilder, collecting: Collecting<Self>) -> ContainerBuilder {
        let _ = collecting;
        builder
    }
}
