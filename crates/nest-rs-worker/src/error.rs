//! The failure an attempt cut at its timeout reports.

use std::time::Duration;

/// An attempt of a worker job that did not end within its timeout, and was cut
/// there. Retryable: the next attempt may meet an answer the first waited on.
///
/// It can grow within 7.x: read its field, and build one through
/// [`JobTimedOut::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobTimedOut {
    /// The timeout the attempt ran past.
    pub timeout: Duration,
}

impl JobTimedOut {
    /// The attempt ran past `timeout`.
    pub const fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl std::fmt::Display for JobTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the attempt did not end within its {:?} timeout, and was cut — the `timeout` key on \
             its job decorator sets it",
            self.timeout
        )
    }
}

impl std::error::Error for JobTimedOut {}
