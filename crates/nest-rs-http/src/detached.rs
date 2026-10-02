//! Work a self-mount runs off the connection that asked for it.
//!
//! The shutdown window closes connections, and an ordinary handler goes with
//! its connection: poem drops the future where it waits. Some surfaces run work
//! a connection only *carries*. rmcp runs every MCP operation on a task of its
//! own, async-graphql runs every DataLoader batch on one, and a WebSocket is
//! served on the task poem hands an upgraded connection to and stops tracking —
//! so cutting the connection, or the transport returning, left that work
//! running on, detached, through the shutdown hooks: writing after
//! `OnModuleDestroy` had run, and filing `ok` for an answer nobody received.
//!
//! A self-mount that runs such work declares a [`DetachedWork`] on its
//! [`HttpEndpointMeta`](crate::HttpEndpointMeta) and runs each unit through it.
//! The transport then carries it down the same two instants as everything it
//! serves:
//!
//! - **at the signal** [`going_away`](DetachedWork::going_away) resolves, and a
//!   unit with no end of its own — a socket, a subscription — ends itself the
//!   way its protocol ends one, while a unit still answering keeps going;
//! - **until the window closes** the transport waits for the work to finish, as
//!   it waits for a request still running;
//! - **at the bound** what still runs is stopped — dropped where it waits — and
//!   the transport waits [`SHUTDOWN_SETTLE_TIMEOUT`] at most, once for every
//!   mount, for it to unwind, so nothing it carried is still running when the
//!   transport returns.
//!
//! The settle is paid **once per transport**, however many mounts declare work:
//! every mount's units are stopped together and waited for together, so the sum
//! a grace period is sized against holds one settle whatever the app mounts.
//!
//! [`SHUTDOWN_SETTLE_TIMEOUT`]: nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT

use std::future::Future;
use std::time::Duration;

use futures_util::future::join_all;
use nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// The units of work a self-mount runs detached from their connection, and
/// the two signals the transport's way down sends them.
///
/// Cheap to clone: every clone shares the signals and the count.
#[derive(Clone, Debug, Default)]
pub struct DetachedWork {
    going_away: CancellationToken,
    stop: CancellationToken,
    running: TaskTracker,
}

impl DetachedWork {
    /// How long a socket the server ends — at the shutdown signal, or at a
    /// lifetime ceiling — gets to take what it is owed: the replies already
    /// queued for it, and its Close frame.
    ///
    /// A peer reading its socket takes them in milliseconds. One that stopped
    /// reading parks the write, and a ceiling is a security control with no
    /// window behind it — so past this the socket is dropped rather than held
    /// open by the peer it was ending. Five seconds: generous to a slow reader,
    /// and well inside the default window, which bounds the same close at
    /// shutdown anyway.
    pub const CLOSE_GRACE: Duration = Duration::from_secs(5);

    /// A fresh set, not yet stopped.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `work` until it settles or the transport stops it: `None` when it
    /// was stopped, and `work` was dropped where it waited.
    ///
    /// **A stopped unit is never polled again**, even one woken in the same
    /// instant. Stopping every mount at once drops units that wait on each
    /// other — a GraphQL field waits on the DataLoader batch beside it — and a
    /// unit polled after its partner was dropped meets a channel nobody will
    /// answer, which async-graphql's loader unwraps.
    pub async fn run<F: Future>(&self, work: F) -> Option<F::Output> {
        let stop = self.stop.clone();
        self.running
            .track_future(async move {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => None,
                    settled = work => Some(settled),
                }
            })
            .await
    }

    /// Resolves at the shutdown signal — when the transport carrying this work
    /// stops accepting and its window opens.
    ///
    /// For a unit with no end of its own: a socket closes with RFC 6455's 1001
    /// Going Away, a subscription completes, and the transport no longer waits
    /// a whole window on something that was never going to finish. A unit
    /// still answering has no use for it — it keeps going, inside the window.
    pub fn going_away(&self) -> impl Future<Output = ()> + Send + use<> {
        self.going_away.clone().cancelled_owned()
    }

    /// A token cancelled when the transport stops this work, for a library that
    /// takes one of its own — rmcp's server config does.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.stop.child_token()
    }

    /// The shutdown signal reached the transport carrying this work.
    pub(crate) fn go_away(&self) {
        self.going_away.cancel();
    }

    /// The transport's half of the way down, for every mount at once: let the
    /// work finish until `bound` — the end of the shutdown window, `None` when
    /// none was opened — then stop what still runs, wait
    /// [`SHUTDOWN_SETTLE_TIMEOUT`] once for all of it, and say per mount what that
    /// cut.
    pub(crate) async fn stop_at(works: &[(String, DetachedWork)], bound: Option<Instant>) {
        for (_, work) in works {
            work.running.close();
        }
        if let Some(bound) = bound {
            let finished = join_all(works.iter().map(|(_, work)| work.running.wait()));
            let _ = tokio::time::timeout_at(bound, finished).await;
        }
        let stopped: Vec<usize> = works
            .iter()
            .map(|(_, work)| {
                let running = work.running.len();
                work.stop.cancel();
                running
            })
            .collect();
        let unwound = join_all(works.iter().map(|(_, work)| work.running.wait()));
        let _ = tokio::time::timeout(SHUTDOWN_SETTLE_TIMEOUT, unwound).await;
        for ((path, work), stopped) in works.iter().zip(stopped) {
            let still_running = work.running.len();
            if stopped > 0 {
                tracing::warn!(
                    target: crate::target::HTTP,
                    path = path.as_str(),
                    stopped,
                    "work a self-mount ran off its connections is stopped with the transport; a \
                     unit still running is dropped unanswered",
                );
            }
            if still_running > 0 {
                tracing::error!(
                    target: crate::target::HTTP,
                    path = path.as_str(),
                    still_running,
                    settle_timeout_ms =
                        u64::try_from(SHUTDOWN_SETTLE_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                    "stopped work did not unwind within its bound; it runs on through the \
                     shutdown hooks",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mounted(path: &str, work: &DetachedWork) -> Vec<(String, DetachedWork)> {
        vec![(path.to_owned(), work.clone())]
    }

    #[tokio::test]
    async fn a_unit_runs_to_its_end_until_stopped_and_is_dropped_where_it_waits_after() {
        let logs = nest_rs_testing::LogCapture::install();
        let work = DetachedWork::new();
        assert_eq!(work.run(async { 7 }).await, Some(7));

        let waiting = tokio::spawn({
            let work = work.clone();
            async move { work.run(std::future::pending::<()>()).await }
        });
        while work.running.is_empty() {
            tokio::task::yield_now().await;
        }
        DetachedWork::stop_at(&mounted("/probe", &work), None).await;

        assert_eq!(waiting.await.expect("the unit's task"), None);
        assert!(work.cancellation_token().is_cancelled());
        let stopped = logs.expect_one(
            crate::target::HTTP,
            "work a self-mount ran off its connections is stopped with the transport; a unit \
             still running is dropped unanswered",
        );
        assert_eq!(stopped.field("stopped").as_deref(), Some("1"));
        logs.expect_none(
            crate::target::HTTP,
            "stopped work did not unwind within its bound; it runs on through the shutdown hooks",
        );
    }

    /// The window is the work's, as it is a request's: a unit that finishes
    /// before the bound is never stopped, and nothing is said about it.
    #[tokio::test(start_paused = true)]
    async fn a_unit_that_finishes_inside_the_window_is_never_stopped() {
        let logs = nest_rs_testing::LogCapture::install();
        let work = DetachedWork::new();
        let finishing = tokio::spawn({
            let work = work.clone();
            async move { work.run(tokio::time::sleep(Duration::from_secs(2))).await }
        });
        while work.running.is_empty() {
            tokio::task::yield_now().await;
        }

        DetachedWork::stop_at(
            &mounted("/probe", &work),
            Some(Instant::now() + Duration::from_secs(20)),
        )
        .await;

        assert_eq!(finishing.await.expect("the unit's task"), Some(()));
        logs.expect_none(
            crate::target::HTTP,
            "work a self-mount ran off its connections is stopped with the transport; a unit \
             still running is dropped unanswered",
        );
    }

    /// The signal reaches the work before the bound does: a unit with no end
    /// of its own ends itself then, and is never stopped.
    #[tokio::test]
    async fn going_away_resolves_at_the_signal_and_not_before() {
        let work = DetachedWork::new();
        let mut leaving = Box::pin(work.going_away());
        assert!(futures_util::poll!(leaving.as_mut()).is_pending());
        work.go_away();
        leaving.await;
        assert!(
            !work.cancellation_token().is_cancelled(),
            "going away is not being stopped",
        );
    }

    /// A unit that never unwinds — its task is never polled again, as when it
    /// blocks its thread — is named at `error` once the settle bound is spent,
    /// because it runs on through the shutdown hooks.
    #[tokio::test(start_paused = true)]
    async fn stopped_work_that_does_not_unwind_is_named_at_error() {
        let logs = nest_rs_testing::LogCapture::install();
        let work = DetachedWork::new();
        let _never_polled = work.running.track_future(std::future::pending::<()>());

        DetachedWork::stop_at(&mounted("/probe", &work), None).await;

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

    /// Two mounts whose units never unwind cost one settle between them, not
    /// one each.
    #[tokio::test(start_paused = true)]
    async fn every_mount_is_stopped_and_waited_for_together() {
        let (first, second) = (DetachedWork::new(), DetachedWork::new());
        let _a = first.running.track_future(std::future::pending::<()>());
        let _b = second.running.track_future(std::future::pending::<()>());

        let started = Instant::now();
        DetachedWork::stop_at(&[("/a".to_owned(), first), ("/b".to_owned(), second)], None).await;

        assert_eq!(started.elapsed(), SHUTDOWN_SETTLE_TIMEOUT);
    }
}
