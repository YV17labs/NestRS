//! Health bridge (feature `health`) — the [`DbHealthIndicator`] that gates
//! readiness on a `DatabaseConnection::ping`.

mod indicator;

pub use indicator::{DatabaseHealthModule, DbHealthIndicator};
