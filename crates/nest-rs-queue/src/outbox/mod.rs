mod config;
mod module;
mod relay;

pub use config::OutboxConfig;
pub use module::{OutboxModule, OutboxSetup};
pub use relay::OutboxRelay;
