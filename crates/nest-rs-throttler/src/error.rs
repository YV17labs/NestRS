//! Typed errors for the throttler port.

/// Why a [`ThrottlerStore`](crate::ThrottlerStore) could not count a hit: the
/// store's own failure, kept opaque so this port names no backend.
///
/// Never `#[error(transparent)]`, whose `source()` skips the store's error
/// itself and drops it from the chain the guard's `warn` renders.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ThrottlerStoreError(#[source] Box<dyn std::error::Error + Send + Sync>);

impl ThrottlerStoreError {
    /// Wrap a store's failure to count — its client's error, a timeout; an
    /// `anyhow::Error` keeps every link.
    pub fn new(source: impl Into<Box<dyn std::error::Error + Send + Sync>> + 'static) -> Self {
        Self(nest_rs_core::boxed_error(source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stores_own_failure_is_the_chain() {
        let refused = ThrottlerStoreError::new(
            anyhow::anyhow!("connection refused").context("the counter's script did not run"),
        );
        assert_eq!(
            nest_rs_core::error_message(&refused),
            "the counter's script did not run: connection refused"
        );
    }
}
