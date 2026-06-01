use crate::container::ContainerBuilder;

/// A statically-composed module: the common case, declared as a unit struct and
/// listed by *type* in `#[module(imports = [...])]`. It carries no per-import
/// configuration, so `register` is an associated fn with no `self`.
///
/// The `#[module]` macro emits this impl, and makes registration **idempotent**:
/// the generated `register` marks the module's `TypeId` via
/// [`ContainerBuilder::mark_registered`] and returns early if it was already
/// registered, so the same module imported through several paths (a diamond)
/// builds its providers exactly once.
pub trait Module {
    /// Build this module's providers (and recurse into imports). Run in the
    /// **register phase**, after every async factory has produced its value.
    fn register(builder: ContainerBuilder) -> ContainerBuilder;

    /// Push the async factories declared by this module's import tree (a
    /// [`DynamicModule`] whose `collect` registers one — e.g. a database pool).
    /// Run in the **collect phase**, before any provider is built, by
    /// [`AppBuilder::build`](crate::AppBuilder::build). The default is a no-op
    /// (a module with no async imports); the `#[module]` macro overrides it to
    /// recurse. Idempotent via [`ContainerBuilder::mark_collected`].
    fn collect(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}

/// A module configured at its import site — the analog of NestJS's
/// `DynamicModule` returned by `forRoot` / `forFeature` / `forRootAsync`.
///
/// Unlike [`Module`], a dynamic module is a **value** that captures options, so
/// `register` takes `self`. A module exposes a `for_root(options)` (or
/// `for_feature(...)`) associated fn that returns such a value; listing that
/// call expression in `#[module(imports = [...])]` registers it:
///
/// ```ignore
/// #[module(imports = [
///     UsersModule,                  // static, by type
///     OpenApiModule::for_root(),    // dynamic, configured at its import site
/// ])]
/// pub struct AppModule;
/// ```
///
/// Dynamic modules are **not** auto-deduplicated: each carries its own config,
/// mirroring NestJS's `forFeature` being called once per feature. A module that
/// must run at most once exposes a [`Module`] impl (the default path) instead.
///
/// A dynamic module participates in **two phases**, both defaulting to a no-op
/// so a kind overrides only what it needs:
///
/// - [`collect`](Self::collect) (collect phase) — register an async
///   [`provide_factory`](ContainerBuilder::provide_factory). This is how a module
///   owns an **asynchronously-built** resource (a database pool, a queue
///   connection) — NestJS's `forRootAsync` — while still being declared in
///   `#[module(imports = [...])]`. `register` is synchronous and cannot `await`,
///   so the factory is collected here and `await`ed by
///   [`AppBuilder::build`](crate::AppBuilder::build) *before* providers are built.
/// - [`register`](Self::register) (register phase) — install synchronous
///   providers, metadata, or config. This is how a module takes **sync** options
///   at its import site (NestJS's `forRoot`).
///
/// A config module overrides `register`; an async-resource module overrides
/// `collect`. The macro calls both uniformly, so the two are indistinguishable
/// at the import site — both are just entries in `imports`.
pub trait DynamicModule {
    /// Register synchronous providers / metadata / config (register phase).
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder
    where
        Self: Sized,
    {
        builder
    }

    /// Register async factories for this module's resources (collect phase).
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}
