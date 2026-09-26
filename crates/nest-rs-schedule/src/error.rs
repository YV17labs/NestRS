//! Typed errors for the schedule port.

/// Why an occurrence could not be claimed: the backend's own failure, kept
/// opaque so this port names no backend.
///
/// Transparent, so the scheduler's `warn` renders the backend's sentence and
/// every cause beneath it rather than a wrapper naming none of them.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct OccurrenceLockError(Box<dyn std::error::Error + Send + Sync>);

impl OccurrenceLockError {
    /// Wrap a backend's failure to claim — a store's error, a timeout.
    pub fn new(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self(source.into())
    }
}
