//! Waiting on what a test cannot await: progress another task makes, a row a
//! worker writes, a line a backend files.

use std::future::Future;
use std::panic::Location;
use std::time::Duration;

/// How often a wait asks again.
const POLL: Duration = Duration::from_millis(10);

/// Poll `ready` until it holds, and fail the test at the caller's line once
/// `within` elapses — so a regression fails where it waited rather than
/// hanging, or passing the wait to fail at a later assertion that names
/// something else. On a paused clock the wait costs nothing.
#[track_caller]
pub fn wait_until(within: Duration, mut ready: impl FnMut() -> bool) -> impl Future<Output = ()> {
    wait_for(within, move || std::future::ready(ready()))
}

/// [`wait_until`] for a condition the test must ask for on every poll — of a
/// database or a broker, most often.
#[track_caller]
pub fn wait_for<F: Future<Output = bool>>(
    within: Duration,
    mut ready: impl FnMut() -> F,
) -> impl Future<Output = ()> {
    let waited_at = Location::caller();
    async move {
        let deadline = tokio::time::Instant::now() + within;
        while !ready().await {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{waited_at}: what it waited for did not hold within {within:?}"
            );
            tokio::time::sleep(POLL).await;
        }
    }
}
