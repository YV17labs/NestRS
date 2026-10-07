//! Typed errors for the job context, and for an attempt cut at its timeout.

use std::time::Duration;

/// A successful job its context could not honour: what went wrong, and whether
/// running the same attempt again could end differently.
///
/// **The classification is the whole point of carrying a value here.** A job's
/// retry replays its body, including every side effect that is not the
/// database's — an HTTP call, an S3 write, a mail. Spending a retry budget on a
/// failure that will repeat identically pays that price several times over and
/// dead-letters anyway, which is why `#[process]` already aborts on its three
/// other deterministic failures (an unsupported wire version, an
/// undeserializable payload, a missing provider).
///
/// A context answers from what the database reported, never from a guess: a
/// serialization failure or a deadlock is retryable, a constraint violation is
/// not, and a commit whose outcome is *unknown* — the connection lost mid-`COMMIT`
/// — is not either. That last one is the deliberate asymmetry: the transaction
/// may have landed, and a framework that replays it turns "may have written
/// once" into "wrote twice". Only a failure the framework knows rolled back is
/// worth repeating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unhonoured {
    /// What the context could not do, as the sentence the transport reports.
    /// `'static` because the database's own error is logged at `error` by the
    /// context that saw it — repeating it in the job's failure would say the
    /// same thing twice, in the place with less of it.
    pub reason: &'static str,
    /// Whether re-running the attempt could produce a different outcome. `false`
    /// is *deterministic*: the retry re-fails identically, having replayed
    /// everything the job body does outside the transaction.
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
/// there. Retryable: an attempt is cut for how long it ran, not for what it
/// read, and the next one may meet an answer the first waited on in vain.
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
