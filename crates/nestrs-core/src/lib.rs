pub mod app;
pub mod config;
pub mod container;
pub mod discoverable;
pub mod discovery;
pub mod error;
pub mod module;
pub mod transport;

pub use app::App;
pub use container::{Container, ContainerBuilder};
pub use discoverable::Discoverable;
pub use discovery::{Discovered, DiscoveryService};
pub use error::{Error, Result};
pub use module::Module;
pub use transport::Transport;

pub use nestrs_macros::{
    controller, graphql_app, injectable, interceptor, module, resolver, routes,
};
