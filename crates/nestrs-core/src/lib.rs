pub mod access;
pub mod app;
pub mod config;
pub mod container;
pub mod discoverable;
pub mod discovery;
pub mod error;
pub mod lifecycle;
pub mod module;
pub mod transport;

pub use access::{AccessGraphError, ModuleDescriptor, ProviderDescriptor};
pub use app::{App, AppBuilder};
pub use container::{Container, ContainerBuilder};
pub use discoverable::Discoverable;
pub use discovery::{Discovered, DiscoveryService};
pub use error::{Error, Result};
pub use lifecycle::{LifecycleHook, LifecyclePhase};
pub use module::{DynamicModule, Module};
pub use transport::Transport;

// Re-exported so `#[hooks]`-generated `inventory::submit!` resolves through the
// framework — apps never depend on `inventory` directly.
pub use inventory;

pub use nestrs_macros::{hooks, injectable, module};
