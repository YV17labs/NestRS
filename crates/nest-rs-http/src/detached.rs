//! Work a self-mount runs off the connection that asked for it.
//!
//! The shutdown window closes connections, and an ordinary handler goes with
//! its connection: poem drops the future where it waits. Some surfaces run work
//! a connection only *carries*. rmcp runs every MCP operation on a task of its
//! own, so cutting the connection that asked for one left the operation running
//! on, detached, through the shutdown hooks — writing after `OnModuleDestroy`
//! had run, and filing `ok` for an answer nobody received.
//!
//! A self-mount that runs such work declares a [`DetachedWork`] on its
//! [`HttpEndpointMeta`](crate::HttpEndpointMeta) and runs each unit through it;
//! the transport stops it once it stops serving, and waits
//! [`SHUTDOWN_SETTLE_TIMEOUT`] at most for what it stopped to unwind, so
//! nothing it carried is still running when the transport returns.
//!
//! [`SHUTDOWN_SETTLE_TIMEOUT`]: nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT

use std::future::Future;

use nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// The units of work a self-mount runs detached from their connection, and
/// the signal that stops them when the transport stops serving.
///
/// Cheap to clone: every clone shares one signal and one count.
#[derive(Clone, Debug, Default)]
pub struct DetachedWork {
    stop: CancellationToken,
    running: TaskTracker,
}

impl DetachedWork {
    /// A fresh set, not yet stopped.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `work` until it settles or the transport stops serving: `None` when
    /// it was stopped, and `work` was dropped where it waited.
    pub async fn run<F: Future>(&self, work: F) -> Option<F::Output> {
        let stop = self.stop.clone();
        self.running
            .track_future(async move {
                tokio::select! {
                    biased;
                    settled = work => Some(settled),
                    () = stop.cancelled() => None,
                }
            })
            .await
    }

    /// A token cancelled when the transport stops serving, for a library that
    /// takes one of its own — rmcp's server config does.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.stop.child_token()
    }

    /// Stop every unit of the self-mount at `path`, wait
    /// [`SHUTDOWN_SETTLE_TIMEOUT`] at most for them to unwind, and say what that
    /// cut. Returns how many were running when stopped, and how many still run
    /// once the wait is over.
    pub(crate) async fn stop(&self, path: &str) -> (usize, usize) {
        let stopped = self.running.len();
        self.stop.cancel();
        self.running.close();
        let _ = tokio::time::timeout(SHUTDOWN_SETTLE_TIMEOUT, self.running.wait()).await;
        let still_running = self.running.len();
        if stopped > 0 {
            tracing::warn!(
                target: crate::target::HTTP,
                path,
                stopped,
                "work a self-mount ran off its connections is stopped with the transport; a \
                 unit still running is dropped unanswered",
            );
        }
        if still_running > 0 {
            tracing::error!(
                target: crate::target::HTTP,
                path,
                still_running,
                settle_timeout_ms =
                    u64::try_from(SHUTDOWN_SETTLE_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                "stopped work did not unwind within its bound; it runs on through the shutdown \
                 hooks",
            );
        }
        (stopped, still_running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_unit_runs_to_its_end_until_stopped_and_is_dropped_where_it_waits_after() {
        let work = DetachedWork::new();
        assert_eq!(work.run(async { 7 }).await, Some(7));

        let waiting = tokio::spawn({
            let work = work.clone();
            async move { work.run(std::future::pending::<()>()).await }
        });
        while work.running.is_empty() {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            work.stop("/probe").await,
            (1, 0),
            "one stopped, none left running"
        );
        assert_eq!(waiting.await.expect("the unit's task"), None);
        assert!(work.cancellation_token().is_cancelled());
    }

    /// A unit that never unwinds — its task is never polled again, as when it
    /// blocks its thread — is named at `error` once the settle bound is spent,
    /// because it runs on through the shutdown hooks.
    #[tokio::test(start_paused = true)]
    async fn stopped_work_that_does_not_unwind_is_named_at_error() {
        let logs = nest_rs_testing::LogCapture::install();
        let work = DetachedWork::new();
        let _never_polled = work.running.track_future(std::future::pending::<()>());

        assert_eq!(work.stop("/probe").await, (1, 1));
        let event = logs.expect_one(
            crate::target::HTTP,
            "stopped work did not unwind within its bound; it runs on through the shutdown hooks",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("path").as_deref(), Some("/probe"));
        assert_eq!(event.field("still_running").as_deref(), Some("1"));
        assert_eq!(
            event.field("settle_timeout_ms"),
            Some(SHUTDOWN_SETTLE_TIMEOUT.as_millis().to_string()),
        );
    }
}
