use crate::container::ContainerBuilder;

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

/// A module configured at its import site — the analog of NestJS's
/// `DynamicModule` returned by `forRoot` / `forFeature` / `forRootAsync`.
///
/// Unlike [`Module`], a dynamic module is a value that captures options:
///
/// ```ignore
/// #[module(imports = [
///     UsersModule,                  // static, by type
///     OpenApiModule::for_root(),    // dynamic, configured at its import site
/// ])]
/// pub struct AppModule;
/// ```
///
/// Dynamic modules are **not** auto-deduplicated — each carries its own
/// config, mirroring NestJS's `forFeature`.
///
/// Two phases, both defaulting to no-op:
///
/// - [`collect`](Self::collect) — queue an async factory (NestJS's
///   `forRootAsync` for resources like a DB pool).
/// - [`register`](Self::register) — install synchronous providers, metadata,
///   or config (NestJS's `forRoot`).
pub trait DynamicModule {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder
    where
        Self: Sized,
    {
        builder
    }

    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}
