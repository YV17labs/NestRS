//! Typed errors for the schedule port.

/// Why an occurrence could not be claimed: the backend's own failure, kept
/// opaque so this port names no backend.
///
/// Displayed as the backend's sentence, and the backend's failure is its
/// `source` — never `#[error(transparent)]`, whose `source()` is the *backend
/// error's* source and skips the error itself: the scheduler's `warn` renders
/// the chain through `error_message`, which reads each link's type, and the
/// first link has to be there to be read.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OccurrenceLockError(#[source] Box<dyn std::error::Error + Send + Sync>);

impl OccurrenceLockError {
    /// Wrap a backend's failure to claim — a store's error, a timeout. Boxed by
    /// [`nest_rs_core::boxed_error`], so an `anyhow::Error` keeps every link.
    pub fn new(source: impl Into<Box<dyn std::error::Error + Send + Sync>> + 'static) -> Self {
        Self(nest_rs_core::boxed_error(source))
    }
}
