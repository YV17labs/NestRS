//! Surface-agnostic nestrs decorators (`#[injectable]`, `#[hooks]`,
//! `#[module]`), re-exported by `nestrs-core`. Each `#[proc_macro_attribute]`
//! entry below is a thin delegation to its implementation module.

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
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    injectable::injectable(args, input)
}

/// Declare application lifecycle hooks on a provider's impl block, mirroring
/// NestJS's lifecycle events.
///
/// Each method tagged with a phase attribute is invoked by
/// [`App`](nestrs_core::App):
///
/// - `#[on_module_init]` / `#[on_application_bootstrap]` — after wiring,
///   before serving. An error aborts boot.
/// - `#[on_module_destroy]` / `#[before_application_shutdown]` /
///   `#[on_application_shutdown]` — after transports stop, best-effort.
///
/// A hook is `async fn(&self)` returning `()` or
/// `Result<(), E: Into<anyhow::Error>>`. Hooks are submitted to a link-time
/// registry, so the provider keeps its single `impl Discoverable`.
#[proc_macro_attribute]
pub fn hooks(args: TokenStream, input: TokenStream) -> TokenStream {
    hooks::hooks(args, input)
}

/// `#[module(imports = [...], providers = [...])]`.
///
/// `imports` is either a type (a static [`Module`](nestrs_core::Module)) or a
/// call expression (a configured [`DynamicModule`](nestrs_core::DynamicModule)
/// — the `forRoot`/`forFeature` analog). `providers` lists what this module
/// declares.
///
/// Each provider entry is `Foo` (a `Discoverable` type) or
/// `Foo as dyn Trait` (a trait-object binding stored via `provide_dyn`).
///
/// Registration is idempotent: a diamond import builds its providers exactly
/// once. Dynamic imports carry their own config and are not deduplicated.
///
/// Imports register first, then providers register via a fixpoint pass — each
/// declares its dependencies through `Discoverable::dependencies` and the
/// macro registers whatever is resolvable, repeating until done. A provider
/// whose dependencies never resolve (missing or cyclic) panics at boot.
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    module::module(args, input)
}
