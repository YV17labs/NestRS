//! The bound every worker job runs within unless it declares its own.

use std::time::Duration;

/// How long one attempt of a worker job — a `#[process]` job, an `#[every]`,
/// `#[cron]` or `#[after]` tick — runs before it is cut, unless its decorator
/// declares `timeout = "…"`: ten minutes, Google Cloud Tasks' dispatch deadline.
pub const JOB_TIMEOUT: Duration = Duration::from_secs(10 * 60);
