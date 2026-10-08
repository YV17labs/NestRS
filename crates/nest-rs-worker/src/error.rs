//! Typed errors for the job context, and for an attempt cut at its timeout.

use std::time::Duration;

/// A successful job its context could not honour: what went wrong, and whether
/// running the same attempt again could end differently.
///
/// A context answers from what the database reported, never from a guess: a
/// serialization failure or a deadlock is retryable; a constraint violation is
/// not, nor a commit whose outcome is *unknown*, which may have landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unhonoured {
    /// What the context could not do, as the sentence the transport reports;
    /// the database's own error is logged by the context that saw it.
    pub reason: &'static str,
    /// Whether re-running the attempt could produce a different outcome.
    pub retryable: bool,
}

impl Unhonoured {
    /// A failure a retry could clear.
    pub const fn retryable(reason: &'static str) -> Self {
        Self {
            reason,
            retryable: true,
        }
    }

    /// A failure a retry would only repeat.
    pub const fn deterministic(reason: &'static str) -> Self {
        Self {
            reason,
            retryable: false,
        }
    }
}

impl std::fmt::Display for Unhonoured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for Unhonoured {}

/// An attempt of a worker job that did not end within its timeout, and was cut
/// there. Retryable: the next attempt may meet an answer the first waited on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobTimedOut {
    /// The timeout the attempt ran past.
    pub timeout: Duration,
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
