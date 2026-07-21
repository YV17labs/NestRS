//! Surface-agnostic nestrs decorators (`#[injectable]`, `#[hooks]`,
//! `#[module]`), re-exported by `nest-rs-core`. Each `#[proc_macro_attribute]`
//! entry below is a thin delegation to its implementation module.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod hooks;
mod injectable;
mod module;

/// Mark a struct as a DI provider built from the container.
///
/// `#[inject]` fields resolve via `container.get()` (or `get_dyn` for
/// `Arc<dyn Trait>`); other fields fall back to `Default::default()`. A struct
/// with no `#[inject]` field uses `<Self as Default>::default()` so a custom
/// `Default` impl is preserved.
///
/// Emits `impl Discoverable for Self` so the struct can appear directly in
/// `#[module(providers = [...])]`.
///
/// `#[injectable(scope = request)]` registers a per-request factory built once
/// per request (and resolved through a `RequestScope` — e.g. the HTTP
/// `Scoped<T>` extractor). A request-scoped provider may depend on singletons
/// but not on other request-scoped providers.
///
/// `#[injectable(scope = transient)]` registers a factory rebuilt on every
/// resolution: each `container.get::<T>()` (or `Scoped<T>` extraction) yields
/// a fresh instance. A transient may depend on singletons and request-scoped
/// providers; a transient depending (transitively) on itself panics with a
/// cycle diagnostic at resolution time.
///
/// # Expands to
///
/// Illustrative sketch (singleton scope):
///
/// ```ignore
/// struct Foo { /* … */ }                       // the item, unchanged
///
/// impl Foo {
///     fn from_container(c: &::nest_rs_core::Container) -> Self { /* resolve #[inject] fields */ }
/// }
///
/// impl ::nest_rs_core::Discoverable for Foo {
///     fn dependencies() -> &'static [TypeId] { /* required #[inject] keys */ }
///     fn dependency_names() -> &'static [&'static str] { /* … */ }
///     fn optional_dependencies() -> &'static [TypeId] { /* Option<Arc<…>> keys */ }
///     fn injected() -> Vec<TypeId> { /* every #[inject] key, for the access graph */ }
///     fn register(b: ContainerBuilder) -> ContainerBuilder {
///         b.provide(Self::from_container(&b.snapshot()))   // singleton: eager value
///     }
/// }
/// ```
///
/// `scope = request` / `scope = transient` keep the same shape but `register`
/// installs a factory (`provide_scoped` / `provide_transient`) instead of an
/// eager value, and report no register-phase `dependencies` (the factory builds
/// lazily); `injected` is still reported for the access graph.
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    injectable::injectable(args, input)
}

/// Declare application lifecycle hooks on a provider's impl block, for the
/// module/application init and shutdown phases.
///
/// Each method tagged with a phase attribute is invoked by
/// `App`:
///
/// - `#[on_module_init]` / `#[on_application_bootstrap]` — after wiring,
///   before serving. An error aborts boot.
/// - `#[on_module_destroy]` / `#[before_application_shutdown]` /
///   `#[on_application_shutdown]` — after transports stop, best-effort.
///
/// A hook is `async fn(&self)` returning `()` or
/// `Result<(), E: Into<anyhow::Error>>`. Hooks are submitted to a link-time
/// registry, so the provider keeps its single `impl Discoverable`.
///
/// # Expands to
///
/// The impl block is re-emitted with the phase attributes stripped; each tagged
/// method gains a free `run` fn plus an `inventory::submit!`. Illustrative
/// sketch for one `#[on_module_init] async fn ready(&self)`:
///
/// ```ignore
/// impl Foo { async fn ready(&self) { /* … */ } }   // phase attr removed
///
/// fn __nestrs_hook_Foo_ready(c: &::nest_rs_core::Container)
///     -> Pin<Box<dyn Future<Output = ::anyhow::Result<()>> + Send + '_>>
/// {
///     Box::pin(async move {
///         match ::nest_rs_core::Container::get::<Foo>(c) {
///             Some(p) => { p.ready().await; Ok(()) }   // Result methods map_err(Into::into)
///             None => Ok(()),
///         }
///     })
/// }
///
/// ::nest_rs_core::inventory::submit! {
///     ::nest_rs_core::LifecycleHook {
///         phase: ::nest_rs_core::LifecyclePhase::OnModuleInit,
///         provider: "Foo", method: "ready",
///         present: |c| Container::get::<Foo>(c).is_some(),
///         run: __nestrs_hook_Foo_ready,
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn hooks(args: TokenStream, input: TokenStream) -> TokenStream {
    hooks::hooks(args, input)
}

/// `#[module(imports = [...], providers = [...])]`.
///
/// `imports` is either a type (a static `Module`) or a
/// call expression (a configured `DynamicModule`
/// built at its import site). `providers` lists what this module
/// declares.
///
/// Each provider entry is `Foo` (a `Discoverable` type) or
/// `Foo as dyn Trait` (a trait-object binding stored via `provide_dyn`).
///
/// Registration is idempotent: a diamond import builds its providers exactly
/// once. Dynamic imports carry their own config and are not deduplicated — but
/// each import expression is **evaluated once**, in the collect phase, and the
/// value it produced is what the register phase installs.
///
/// Imports register first, then providers register via a fixpoint pass — each
/// declares its dependencies through `Discoverable::dependencies` and the
/// macro registers whatever is resolvable, repeating until done. A provider
/// whose dependencies never resolve (missing or cyclic) panics at boot.
///
/// # Expands to
///
/// `impl Module` (a `register` fixpoint loop + a `collect` async-factory pass)
/// plus a link-time `ModuleDescriptor` for the access graph. Illustrative
/// sketch:
///
/// ```text
/// struct AppModule;                                  // the item, unchanged
///
/// impl ::nest_rs_core::Module for AppModule {
///     fn register(mut b: ContainerBuilder) -> ContainerBuilder {
///         if !b.mark_registered(TypeId::of::<AppModule>()) { return b; }   // dedup diamonds
///         b = <SomeImport as Module>::register(b);                          // imports first
///         // fixpoint: each provider registers once its deps are present —
///         //   <Foo as Discoverable>::register(b)            (concrete)
///         //   b.provide_dyn::<Trait>(Arc::new(Foo::from_container(&b.snapshot())))  (`as dyn Trait`)
///         // stalls panic naming the unprovided / cyclic provider
///         ::nest_rs_core::__module_registered("AppModule");
///         b
///     }
///     fn collect(mut b: ContainerBuilder) -> ContainerBuilder {
///         /* queue each import's async factories */ b
///     }
/// }
///
/// ::nest_rs_core::inventory::submit! {
///     ::nest_rs_core::ModuleDescriptor { module, name, imports: &[…], providers: &[…] }
/// }
/// ```
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    module::module(args, input)
}
