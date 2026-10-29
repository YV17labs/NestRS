//! Typed errors for the throttler port.

/// Why a [`ThrottlerStore`](crate::ThrottlerStore) could not count a hit: the
/// store's own failure, kept opaque so this port names no backend.
///
/// Never `#[error(transparent)]`, whose `source()` skips the store's error
/// itself and drops it from the chain the denial's line renders.
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

/// Why a hit was refused without being counted: the store named could not
/// count it — what a throttler denial keeps as its cause, naming the store and
/// never the key, which holds the caller's address.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum UncountedError {
    /// The store answered an error.
    #[error("the throttler store `{store}` failed")]
    Failed {
        /// The store, as it names itself.
        store: &'static str,
        /// Its failure.
        #[source]
        error: ThrottlerStoreError,
    },
    /// The store did not answer within the guard's bound.
    #[error("the throttler store `{store}` did not answer within {waited_ms} ms")]
    Silent {
        /// The store, as it names itself.
        store: &'static str,
        /// The bound waited out.
        waited_ms: u64,
    },
    /// The store panicked; the panic is filed on its own line.
    #[error("the throttler store `{store}` panicked")]
    Panicked {
        /// The store, as it names itself.
        store: &'static str,
    },
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
