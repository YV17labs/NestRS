//! Surface-agnostic nestrs decorators (`#[injectable]`, `#[interceptor]`,
//! `#[module]`, `#[hooks]`, `#[input]`, `#[instance]`, `#[main]`), re-exported
//! by `nest-rs-core`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod entry;
mod hooks;
mod injectable;
mod input;
mod instance;
mod interceptor;
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

/// Declares a struct as an interceptor a module brings for every unit it
/// serves: a singleton provider, built like an `#[injectable]` one, that each
/// site composes from the container once.
///
/// One key, `stage = handler | unit` (default `handler`):
///
/// - `handler` runs it after the guards and the class gate, after the
///   interceptors the app lists, in module import order.
/// - `unit` runs it around the guards, so it sees a denial too — an opening,
///   written in the source.
///
/// Its order within its stage is its `Layer::priority`; a type the app also
/// lists runs once.
#[proc_macro_attribute]
pub fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(interceptor::interceptor(args, input).into()).into()
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
/// A binding takes a singleton — an edge host, or a `scope = request` or
/// `scope = transient` provider, bound so is a compile error — and holds one
/// `Foo` under its own type and each trait it is listed with. A provider is
/// listed once: a bare `Foo` beside its binding, or an entry written twice, is
/// a compile error.
///
/// A diamond import builds its providers once; a dynamic import is not
/// deduplicated, and its expression is evaluated once. Providers register in
/// dependency order, whatever their order in the list. A missing dependency
/// fails the boot with a `MissingDependencyError` / `AccessGraphError`, a
/// provider cycle with a `ProviderCycleError`, and a provider whose
/// dependency nothing had registered yet when its module built it — across an
/// import cycle, or registered later by a hand-written module — with an
/// `UnbuiltProviderError`.
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

/// Declares a unit struct as an `Instance` marker: one instance of a resource
/// an app opens more than once, told apart by this type.
///
/// Its name is the ident in snake case (`ReadReplica` reads `read_replica`),
/// inserted after its config's vendor (`<PREFIX>_REDIS__READ_REPLICA__*`).
/// Takes no argument; refuses a struct with fields or generics, `Unnamed`, a
/// name holding `__` and the layout's reserved words. Emits the `Instance`
/// impl and a link-time declaration, so two markers reading one name are
/// refused at first use.
#[proc_macro_attribute]
pub fn instance(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(instance::instance(args.into(), input.into())).into()
}

/// The binary's entry point, on an `async fn`: runs the body on tokio's
/// multi-threaded runtime (every driver enabled, sized by
/// `TOKIO_WORKER_THREADS`), then shuts the runtime down within what is left of
/// the shutdown hooks' budget, abandoning and logging a blocking task still
/// running rather than waiting on it.
///
/// It installs the `SIGINT` / `SIGTERM` handlers before the body runs, so the
/// first signal stops the boot at its next await or drains the serving app,
/// and one received before any app exists exits at once with 130 / 143.
///
/// Takes no argument. The app's manifest needs no `tokio` line for it.
#[proc_macro_attribute]
pub fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(entry::main(args, input).into()).into()
}
