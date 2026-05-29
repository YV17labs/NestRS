//! The surface-agnostic nestrs decorators: `#[injectable]` (DI provider) and
//! `#[module]` (composition + order-independent registration). Re-exported by
//! `nestrs-core`. Surface-specific decorators live with their surface
//! (`nestrs-http`, `nestrs-graphql`, `nestrs-mcp`); shared token helpers live
//! in `nestrs-codegen`.
//!
//! The `#[proc_macro_attribute]` entry functions live here (the language forces
//! them to the crate root); each is a thin delegation to its implementation
//! module (`injectable`, `hooks`, `module`).

use proc_macro::TokenStream;

mod hooks;
mod injectable;
mod module;

/// Mark a struct as a provider that can be constructed from the IoC container.
///
/// - Fields tagged `#[inject]` are resolved via `container.get()`.
/// - Other fields fall back to `Default::default()`.
/// - If no field carries `#[inject]`, the macro defers to `<Self as Default>::default()`
///   so any custom `Default` impl on the struct is preserved.
///
/// Also emits `impl Discoverable for Self` so the struct is usable directly
/// in `#[module(providers = [...])]`. The registration simply builds the
/// value via `from_container` and stores it via `ContainerBuilder::provide`.
///
/// `#[injectable(scope = request)]` makes the provider **request-scoped**: it is
/// not built as a singleton but registered as a per-request factory
/// (`ContainerBuilder::provide_scoped`), built fresh for — and cached within —
/// each request, resolved through a `RequestScope` (e.g. the HTTP `Scoped<T>`
/// extractor). Like a controller it is built lazily, so its register-phase
/// `dependencies` are empty while `injected` still reports its `#[inject]` keys
/// for the access-graph check. Its dependencies resolve from the singleton root,
/// so it may inject singletons but not other request-scoped providers. The
/// default, `scope = singleton`, is the plain shared provider.
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    injectable::injectable(args, input)
}

/// Declare application lifecycle hooks on a provider's impl block, mirroring
/// NestJS's lifecycle events.
///
/// Applied to an `impl` block of an `#[injectable]` provider. Each method tagged
/// with a phase attribute is invoked by [`App`](nestrs_core::App) at that point:
///
/// - `#[on_module_init]` / `#[on_application_bootstrap]` — after the container
///   is built and transports configured, before serving. An error aborts boot.
/// - `#[on_module_destroy]` / `#[before_application_shutdown]` /
///   `#[on_application_shutdown]` — after the transports stop, best-effort.
///
/// A hook is `async fn(&self)` returning either nothing or
/// `Result<(), E: Into<anyhow::Error>>`. The macro resolves the provider from
/// the container at call time — the same instance request handlers see — and
/// submits each hook to a link-time registry, so there is no central list and
/// the provider keeps its single `impl Discoverable` (emitted by
/// `#[injectable]`). Like the verb attributes of `#[routes]`, the phase
/// attributes are consumed here and need no import.
#[proc_macro_attribute]
pub fn hooks(args: TokenStream, input: TokenStream) -> TokenStream {
    hooks::hooks(args, input)
}

/// `#[module(imports = [...], providers = [...])]`.
///
/// Both keys are optional. `imports` lists other modules to compose in, each
/// contributing its own providers and metadata. An import is either:
///
/// - a **type** (`UsersModule`) — a static [`Module`](nestrs_core::Module),
///   composed via `Module::register`, or
/// - a **call expression** (`OpenApiModule::for_root(opts)`) — a configured
///   [`DynamicModule`](nestrs_core::DynamicModule) value, composed via
///   `DynamicModule::register`. This is how a module receives runtime options
///   at its import site, the analog of NestJS's `forRoot`/`forFeature`.
///
/// `providers` lists everything this module declares — services, controllers,
/// interceptors, cron jobs / event handlers / MCP tools.
///
/// Registration is **idempotent**: the generated `Module::register` marks the
/// module's `TypeId` and returns early if it was already registered, so a
/// module pulled in through several import paths (a diamond) builds its
/// providers exactly once. (Dynamic-module imports carry their own config and
/// are deliberately not deduplicated.)
///
/// Each provider entry is one of:
///
/// - `Foo` — a concrete type that implements `Discoverable` (every
///   `#[injectable]`, `#[controller]`+`#[routes]`, and `#[interceptor]`
///   struct does). The macro expands to a single
///   `<Foo as Discoverable>::register(builder)` call.
/// - `Foo as dyn Trait` — a trait-object binding. The macro builds `Foo`
///   from a snapshot and stores it under the trait's `TypeId` via
///   `provide_dyn`, so dependents can inject `Arc<dyn Trait>`.
///
/// Order does not matter. Imports register first, then providers register by
/// a fixpoint pass: each provider declares its dependencies via
/// `Discoverable::dependencies`, and the macro registers whatever is
/// resolvable, repeating until everything is in. A provider whose
/// dependencies never become available — missing from this module and its
/// imports, or part of a cycle — panics at boot with the offending names.
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    module::module(args, input)
}
