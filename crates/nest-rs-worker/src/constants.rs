//! The bound every worker job runs within unless it declares its own.

use std::time::Duration;

/// How long one attempt of a worker job — a `#[process]` job, an `#[every]`,
/// `#[cron]` or `#[after]` tick — runs before it is cut, unless its decorator
/// declares `timeout = "…"`: ten minutes, Google Cloud Tasks' dispatch deadline.
/// The worker awaits developer code with no edge deadline above it, so a call
/// that never answers — an outbound request with no timeout of its own — would
/// otherwise hold its permit, or its schedule, for as long as the process lives.
pub const JOB_TIMEOUT: Duration = Duration::from_secs(10 * 60);
