//! Health bridge (feature `health`) — the [`SeaOrmHealthIndicator`] that gates
//! readiness on a `DatabaseConnection::ping`, and the [`SeaOrmHealthModule`]
//! import seam that registers it.

mod indicator;
mod module;

pub use indicator::SeaOrmHealthIndicator;
pub use module::SeaOrmHealthModule;
