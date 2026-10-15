//! The bound every worker job's body runs within unless its decorator declares
//! its own, and the failure an attempt cut at it reports.

use std::time::Duration;

/// How long the body of one attempt of a worker job — a `#[process]` job, an
/// `#[every]`, `#[cron]` or `#[after]` tick — runs before it is cut, unless its
/// decorator declares `timeout = "…"`: ten minutes, Google Cloud Tasks'
/// dispatch deadline.
pub const JOB_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// An attempt of a worker job whose body did not end within its timeout, and
/// was cut there: its unit of work rolled back, so nothing it wrote stands.
/// Retryable: the next attempt may meet an answer the first waited on.
///
/// The timeout bounds the body alone: a body that returned in time is never
/// this, however long its commit takes.
///
/// It can grow within 7.x: read its field, and build one through
/// [`JobTimedOut::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobTimedOut {
    /// The timeout the attempt's body ran past.
    pub timeout: Duration,
}

impl JobTimedOut {
    /// The attempt's body ran past `timeout`.
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
