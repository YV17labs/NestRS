//! Health bridge (feature `health`) — the [`DbHealthIndicator`] that gates
//! readiness on a `DatabaseConnection::ping`, and the [`SeaOrmHealthModule`]
//! import seam that registers it.

mod indicator;
mod module;

pub use indicator::DbHealthIndicator;
pub use module::SeaOrmHealthModule;
