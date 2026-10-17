//! Typed errors for the schedule port.

/// Why an occurrence could not be claimed: the backend's own failure, kept
/// opaque so this port names no backend.
///
/// Never `#[error(transparent)]`, whose `source()` skips the backend's error
/// itself and drops it from the chain the scheduler's `warn` renders.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OccurrenceLockError(#[source] Box<dyn std::error::Error + Send + Sync>);

impl OccurrenceLockError {
    /// Wrap a backend's failure to claim — a store's error, a timeout; an
    /// `anyhow::Error` keeps every link.
    pub fn new(source: impl Into<Box<dyn std::error::Error + Send + Sync>> + 'static) -> Self {
        Self(nest_rs_core::boxed_error(source))
    }
}
