//! Surface-agnostic nestrs decorators (`#[injectable]`, `#[module]`,
//! `#[hooks]`, `#[input]`, `#[main]`), re-exported by `nest-rs-core`. Each
//! `#[proc_macro_attribute]` entry below is a thin delegation to its
//! implementation module.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod entry;
mod hooks;
mod injectable;
mod input;
mod module;

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
/// It also emits `impl ProviderResidency`: `true` for a singleton, `false` for
/// `request` and `transient`.
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(injectable::injectable(args, input).into()).into()
}

/// Each method tagged with a phase attribute is invoked by `App`:
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
    ::nest_rs_codegen::reroot(hooks::hooks(args, input).into()).into()
}

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
/// whose dependencies never resolve fails the boot — a **missing** dependency is
/// deferred to the access-graph check and surfaces as a named
/// `MissingDependencyError` / `AccessGraphError` through the same `Result` every
/// other wiring failure takes, and only a true provider **cycle**, invisible to
/// that graph, still panics.
///
/// The struct gains an `impl Module` and a hidden link-time descriptor the
/// boot's access-graph check reads.
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(module::module(args, input).into()).into()
}

/// `#[input]` — the wire-DTO shorthand. Appends `Serialize`, `Deserialize`,
/// `Validate` and `JsonSchema`, each **routed through the framework** rather
/// than named at the call site, plus `#[serde(deny_unknown_fields)]` so an
/// unknown field on the wire (e.g. `is_admin: true`) is rejected at parse time
/// instead of silently dropped, and the DTO documents itself in the OpenAPI
/// document without a second derive to remember.
///
/// The routing is the whole point and it used to be missing from this page: a
/// derive expands against the **call site's** prelude, so a derive path rooted
/// there would oblige the DTO's crate to declare `serde`, `validator` and
/// `schemars` — the three manifest lines this decorator exists to absorb. Each
/// derive therefore carries its `crate = ` override to the same framework path.
///
/// `Serialize` is in the list because a wire DTO travels both ways: the same
/// type is returned as `Json<T>` from a handler. Adding a manual
/// `#[derive(serde::Serialize)]` next to `#[input]` is therefore a conflicting
/// impl (`E0119`), not a top-up — the shorthand already carries it.
#[proc_macro_attribute]
pub fn input(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(input::input(args, item).into()).into()
}

/// It builds what `#[tokio::main]` builds — tokio's multi-threaded runtime with
/// every driver enabled, sized by `TOKIO_WORKER_THREADS` — runs the body on it,
/// and then does the one thing `#[tokio::main]` cannot: it tears the runtime
/// down within what the shutdown hooks and the telemetry flush left of the
/// hooks' budget — all of it when the body ran no app. Dropping a runtime waits
/// for every blocking task still running, so under `#[tokio::main]` a hook the
/// budget abandoned while it waited on a `spawn_blocking` — or a request dropped
/// at the shutdown window mid `tokio::fs` call — held the process past every
/// bound, after its last line.
/// Here what is still running then is abandoned with the process, and said.
///
/// No argument is taken, and one is refused naming why: there is nothing left
/// for one to choose. The app's manifest needs no `tokio` line for it.
#[proc_macro_attribute]
pub fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(entry::main(args, input).into()).into()
}
