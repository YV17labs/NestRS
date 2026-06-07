pub mod access;
pub mod app;
pub mod container;
pub mod discoverable;
pub mod discovery;
pub mod job;
pub mod layer;
pub mod lifecycle;
pub mod module;
pub mod scope;
pub mod transport;

pub use access::{
    AccessGraphError, ModuleDescriptor, ProviderDescriptor, ReachableProviders, ResolverDescriptor,
    ResolverSchemaActive, UnreachableResolversError,
};
pub use app::{App, AppBuilder};
pub use container::{Container, ContainerBuilder};
pub use discoverable::Discoverable;
pub use discovery::{Discovered, DiscoveryService};
pub use job::{JobContext, run_in_job_context};
pub use layer::{Layer, LayerKind, LayerScope, Public};
pub use lifecycle::{LifecycleHook, LifecyclePhase};
pub use module::{__module_registered, DynamicModule, Module};
pub use scope::RequestScope;
pub use transport::{Transport, TransportContribution};

// Re-exported so `#[hooks]`-generated `inventory::submit!` resolves through the
// framework — apps never depend on `inventory` directly.
pub use inventory;

pub use nest_rs_macros::{hooks, module};

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
pub use nest_rs_macros::injectable;
