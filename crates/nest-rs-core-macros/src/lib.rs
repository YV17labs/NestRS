//! Surface-agnostic nestrs decorators (`#[injectable]`, `#[module]`,
//! `#[hooks]`, `#[input]`, `#[main]`), re-exported by `nest-rs-core`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod entry;
mod hooks;
mod injectable;
mod input;
mod module;

/// Declares a struct as a provider: `#[inject]` fields resolve via
/// `container.get()` (or `get_dyn` for `Arc<dyn Trait>`), other fields fall
/// back to `Default::default()`; with no `#[inject]` field, the struct's own
/// `Default` impl builds it.
///
/// Emits `impl Discoverable` (so the struct can sit in
/// `#[module(providers = [...])]`) and `impl ProviderResidency` (`true` for a
/// singleton, `false` for `request` and `transient`).
///
/// One key, `scope = singleton | request | transient` (default `singleton`):
///
/// - `request` builds once per request, resolved through a `RequestScope` (e.g.
///   the HTTP `Scoped<T>` extractor); it may depend on singletons and on other
///   request-scoped providers, one instance each per request, and no singleton
///   may depend on it.
/// - `transient` builds a fresh instance on every resolution; it may depend on
///   singletons and request-scoped providers, and one depending (transitively)
///   on itself panics with a cycle diagnostic at resolution time.
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(injectable::injectable(args, input).into()).into()
}

/// Registers a provider's lifecycle hooks; `App` invokes each method tagged
/// with a phase attribute:
///
/// - `#[on_module_init]` / `#[on_application_bootstrap]` — after wiring,
///   before serving. An error aborts boot.
/// - `#[on_module_destroy]` / `#[before_application_shutdown]` /
///   `#[on_application_shutdown]` — after transports stop, best-effort.
///
/// A hook is `async fn(&self)` returning `()` or
/// `Result<(), E: Into<anyhow::Error>>`; one phase per method, and the phase
/// attribute takes no argument. `#[hooks]` itself takes none.
#[proc_macro_attribute]
pub fn hooks(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(hooks::hooks(args, input).into()).into()
}

/// Declares a module, with two keys, each taking a list, each written once:
/// `imports` holds types (a static `Module`) or call expressions (a configured
/// `DynamicModule`, e.g. `HttpModule::for_root(None)`); `providers` holds
/// `Foo` (a `Discoverable` type) or `Foo as dyn Trait` (a trait-object binding).
///
/// A diamond import builds its providers once; a dynamic import is not
/// deduplicated, and its expression is evaluated once. Providers register in
/// dependency order, whatever their order in the list. A missing dependency
/// fails the boot with a `MissingDependencyError` / `AccessGraphError`, a
/// provider cycle with a `ProviderCycleError`.
///
/// Emits `impl Module` and a hidden link-time descriptor the boot's
/// access-graph check reads.
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(module::module(args, input).into()).into()
}

/// The wire-DTO shorthand: derives `Serialize`, `Deserialize`, `Validate` and
/// `JsonSchema` through the framework (the DTO's crate needs no `serde`,
/// `validator` or `schemars` line), plus `#[serde(deny_unknown_fields)]`, so an
/// unknown field on the wire (e.g. `is_admin: true`) is rejected at parse time.
///
/// Takes no argument, and only a struct with named fields. Other derives
/// (`Debug`, `Clone`) may be added beside it; deriving one of the four again is
/// a conflicting impl (`E0119`).
#[proc_macro_attribute]
pub fn input(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(input::input(args, item).into()).into()
}

/// The binary's entry point, on an `async fn`: runs the body on tokio's
/// multi-threaded runtime (every driver enabled, sized by
/// `TOKIO_WORKER_THREADS`), then shuts the runtime down within what is left of
/// the shutdown hooks' budget, abandoning and logging a blocking task still
/// running rather than waiting on it.
///
/// Takes no argument. The app's manifest needs no `tokio` line for it.
#[proc_macro_attribute]
pub fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(entry::main(args, input).into()).into()
}
