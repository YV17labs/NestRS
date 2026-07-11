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
    AccessGraphError, KeyedDependencyError, ModuleDescriptor, ProviderDescriptor,
    ReachableProviders, ResolverDescriptor, ResolverSchemaActive, UnreachableResolversError,
    validate_keyed_access_graph,
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
