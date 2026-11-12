//! What both of this crate's suites stand in front of — or in place of — a
//! Redis: [`connection`]'s scripted server and the shape of a refusal, and
//! [`tls`]'s terminating proxy. Each suite's `main.rs` includes it by path; an
//! item here is one both suites use, or the suite that does not would hold
//! dead code.

pub(crate) mod connection;
pub(crate) mod tls;

use nest_rs_testing::LogCapture;

/// What the boot says before each retry of an attempt that may clear.
const RETRYING: &str = "redis unreachable — retrying within the connect budget";

/// A refusal is one connection attempt, never a budget spent retrying one: the
/// boot `logs` heard announced no retry, however long its attempt took.
pub(crate) fn no_attempt_retried(logs: &LogCapture, case: &str) {
    let retried = logs.find(nest_rs_redis::TARGET, RETRYING);
    assert!(
        retried.is_empty(),
        "{case}: a refusal spends none of the budget retrying: {retried:#?}"
    );
}
