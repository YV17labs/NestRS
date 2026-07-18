//! The nestrs core: the IoC container, the module system, the boot-time access
//! graph, and the application lifecycle. Every other `nest-rs-*` crate composes
//! on these primitives; an app rarely calls into this crate beyond
//! [`App::builder`] in `main`.
//!
//! # The composition model
//!
//! An [`App`] is built from a root [`Module`]. A `#[module]` declares the
//! `providers` it owns and the `imports` it depends on; a *provider* is any
//! injectable value — a service, a controller, a guard — that names its
//! dependencies with `#[inject]`. The container is a single flat registry keyed
//! by `TypeId`. Visibility is Rust's job, not a per-module export list: expose a
//! `pub trait`, bind an implementation with `provide_dyn`, and consumers inject
//! `Arc<dyn Trait>`.
//!
//! # Wiring is checked, not reflected
//!
//! The dependency graph is validated at boot, never resolved by reflection at
//! runtime. [`validate_access_graph`](crate::access::validate_access_graph)
//! walks the module tree from the root and fails with a named error before any
//! transport starts: [`AccessGraphError`] when a provider reaches across a
//! module boundary no `import` covers, [`MissingDependencyError`] when a
//! dependency no module provides would otherwise panic at first resolution. A
//! misconfigured import is a startup error naming the fix, not a `Cannot
//! resolve` on the first request. `#[use_guards]` / `#[use_filters]` /
//! `#[use_interceptors]` are checked the same way.
//!
//! # Scopes and lifecycle
//!
//! Providers are singletons by default. `#[injectable(scope = request)]` builds
//! one instance per request, reached through a transport's request boundary;
//! `#[injectable(scope = transient)]` rebuilds on every resolution. Lifecycle
//! hooks (`#[on_module_init]`, `#[on_application_bootstrap]`,
//! `#[on_module_destroy]`, …) run per phase as [`App::run`] drains them —
//! init failure aborts boot, shutdown is best-effort.
//!
//! # Discovery
//!
//! Module-wired items implement [`Discoverable`] and are found through link-time
//! `inventory`, gated on reachability from the running app's root
//! ([`ReachableProviders`]). An item linked into the binary but living in no
//! reachable module is inert, with a boot `warn` — which is what lets one shared
//! feature crate serve different per-binary subsets (an API mounts HTTP and
//! GraphQL; a worker mounts only the queue).
//!
//! ```ignore
//! use std::sync::Arc;
//! use nest_rs_core::{App, injectable, module};
//!
//! #[injectable]
//! #[derive(Default)]
//! struct GreetingService;
//!
//! #[module(providers = [GreetingService])]
//! struct AppModule;
//!
//! // Boot fails here with a named error if the graph is misconfigured.
//! let app = App::new::<AppModule>()?;
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod access;
pub mod app;
pub mod container;
pub(crate) mod cycle_guard;
pub mod discoverable;
pub mod discovery;
pub mod layer;
pub mod layer_chain;
pub mod lifecycle;
pub mod metadata;
pub mod module;
pub mod request_scope;
pub mod transport;

pub use access::{
    AccessError, AccessGraphError, KeyedDependencyError, MissingDependencyError, ModuleDescriptor,
    ProviderDescriptor, ReachableProviders, ResolverDescriptor, ResolverSchemaActive,
    UnreachableResolversError, validate_keyed_access_graph,
};
pub use app::{App, AppBuilder};
pub use container::{Container, ContainerBuilder, KeyedDependency, ProviderKey};
pub use discoverable::Discoverable;
pub use discovery::{AccessGraphSnapshot, Discovered, DiscoveryService};
pub use layer::{Layer, LayerKind, LayerSite};
pub use layer_chain::{ResolvedLayer, compose_chain, dedup_bucket};
pub use lifecycle::{LifecycleHook, LifecyclePhase};
pub use metadata::{HandlerMetadata, MappedError, Public};
pub use module::{__module_registered, DynamicModule, Module};
pub use request_scope::RequestScope;
pub use transport::{Transport, TransportContribution};

// Re-exported so `#[hooks]`-generated `inventory::submit!` resolves through the
// framework — apps never depend on `inventory` directly.
pub use inventory;

pub use nest_rs_core_macros::{hooks, module};

/// The provider decorator. Every `#[inject]` field must be an `Arc<T>` or
/// `Arc<dyn Trait>` — a dependency is resolved from the container as a shared
/// `Arc` — so a non-`Arc` injected field is rejected at compile time rather than
/// failing with a cryptic type error in generated code:
///
/// ```compile_fail
/// use nest_rs_core::injectable;
///
/// #[injectable]
/// struct Bad {
///     #[inject]
///     dep: u32, // not an `Arc` — compile error
/// }
/// ```
pub use nest_rs_core_macros::injectable;
