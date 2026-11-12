//! [`SessionClient`], what the mount knows of one session's client — when it
//! last spoke, when it and its session last exchanged a message, the streams it
//! holds — and its watch, which ends the session once the client goes quiet past
//! what the mount allows: the idle timeout, or the ping's while it listens.
//!
//! The mount keeps both bounds itself, rmcp's idle timer off: that timer counts
//! every message of the session, the server's own included while no stream can
//! carry them, and it stops with rmcp's worker, which a write to a stream nobody
//! reads holds (its channel is bounded). The watch times both off the streams,
//! and ending a session lets go of every stream it holds, which frees that write.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::ping::Pings;
use crate::session_stream::HeldStream;

/// What the mount knows of one session's client.
pub(crate) struct SessionClient {
    /// When the session opened: the instants below count from it.
    opened: Instant,
    /// Nanoseconds from `opened` to the client's last message, of any kind.
    heard: AtomicU64,
    /// Nanoseconds from `opened` to the last use of the session: a message of
    /// the client's but a ping's answer, one of the server's a stream carried
    /// to it, or the end of a call's wait for its answer.
    used: AtomicU64,
    /// Every stream of the session the transport holds.
    streams: Mutex<Vec<Arc<HeldStream>>>,
    /// Wakes the watch when a stream the client listens on opens, which may
    /// bring its deadline forward.
    listened: Notify,
    /// Whether the session was ended for its client's silence: said once.
    gone: AtomicBool,
    /// Ends the session: its worker stops, its place frees.
    end: CancellationToken,
}

impl SessionClient {
    /// A client heard, and its session used, as the session opens; `end` ends
    /// the session.
    pub(crate) fn new(end: CancellationToken) -> Self {
        Self {
            opened: Instant::now(),
            heard: AtomicU64::new(0),
            used: AtomicU64::new(0),
            streams: Mutex::new(Vec::new()),
            listened: Notify::new(),
            gone: AtomicBool::new(false),
            end,
        }
    }

    /// Nanoseconds from `opened` to now.
    fn since_opened(&self) -> u64 {
        let since = Instant::now().saturating_duration_since(self.opened);
        u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
    }

    /// A message of any kind came from the client: it is there.
    pub(crate) fn heard(&self) {
        self.heard.fetch_max(self.since_opened(), Ordering::Relaxed);
    }

    /// The session was used now.
    pub(crate) fn used(&self) {
        self.used.fetch_max(self.since_opened(), Ordering::Relaxed);
    }

    /// When the client's last message arrived.
    pub(crate) fn heard_at(&self) -> Instant {
        self.opened + Duration::from_nanos(self.heard.load(Ordering::Relaxed))
    }

    /// When the session was last used.
    fn used_at(&self) -> Instant {
        self.opened + Duration::from_nanos(self.used.load(Ordering::Relaxed))
    }

    /// Whether the session was ended: rmcp may still hold it, until it stops
    /// serving it.
    pub(crate) fn is_closed(&self) -> bool {
        self.end.is_cancelled()
    }

    /// The streams the session holds, however a holder left the lock.
    fn streams(&self) -> std::sync::MutexGuard<'_, Vec<Arc<HeldStream>>> {
        self.streams.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hold `stream` for the session, until [`let_go`](Self::let_go) or the
    /// session's end.
    pub(crate) fn hold(&self, stream: Arc<HeldStream>) {
        let listening = stream.is_listening();
        {
            // Read under the lock `close` takes the streams under, after it
            // cancels: a stream is held before the session ends, or let go of.
            let mut streams = self.streams();
            if self.end.is_cancelled() {
                drop(streams);
                stream.release();
                return;
            }
            streams.push(stream);
        }
        if listening {
            self.listened.notify_one();
        }
    }

    /// The transport dropped `stream`.
    pub(crate) fn let_go(&self, stream: &Arc<HeldStream>) {
        self.streams().retain(|held| !Arc::ptr_eq(held, stream));
    }

    /// Whether the client listens on a stream of the session, which asks it
    /// pings; and whether a call of it waits on its answer, which is use.
    fn holding(&self) -> (bool, bool) {
        let streams = self.streams();
        (
            streams.iter().any(|held| held.is_listening()),
            streams.iter().any(|held| held.is_awaited()),
        )
    }

    /// End the session, letting go of every stream it holds, so a write of its
    /// worker's to a stream nobody reads gives way.
    pub(crate) fn close(&self) {
        self.end.cancel();
        let streams = std::mem::take(&mut *self.streams());
        for held in streams {
            held.release();
        }
    }

    /// End the session of a client that answered nothing, said once for the
    /// mount `pings` names.
    pub(crate) fn give_up(&self, pings: &Pings) {
        if !self.gone.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: crate::TARGET,
                host = crate::operation::REQUEST.handler().host_name(),
                path = pings.path(),
                ping_timeout_ms = pings.timeout().as_millis(),
                "mcp session closed: the client sent nothing back within the ping timeout",
            );
        }
        self.close();
    }

    /// End the session once its client goes quiet past what the mount allows:
    /// `idle` unused, or, while it listens on a stream, the silence `pings`
    /// allow, timed off the stream whether or not the transport could write
    /// it a ping. Returns once the session ends, however it ends.
    pub(crate) async fn watch(self: Arc<Self>, idle: Duration, pings: Pings) {
        let silence = pings.silence();
        let deadline = tokio::time::sleep(Duration::ZERO);
        tokio::pin!(deadline);
        loop {
            let now = Instant::now();
            let (listening, awaited) = self.holding();
            let silent_at = silence
                .filter(|_| listening)
                .map(|silence| self.heard_at() + silence);
            if silent_at.is_some_and(|at| at <= now) {
                self.give_up(&pings);
                return;
            }
            // A call waiting on its answer suspends the idle timeout; the
            // watch looks again an `idle` later, before the end of that wait
            // can bring the deadline closer.
            let idle_at = if awaited {
                now + idle
            } else {
                let at = self.used_at() + idle;
                if at <= now {
                    self.close();
                    return;
                }
                at
            };
            deadline
                .as_mut()
                .reset(silent_at.map_or(idle_at, |at| at.min(idle_at)));
            tokio::select! {
                () = self.end.cancelled() => return,
                () = self.listened.notified() => {}
                () = &mut deadline => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use rmcp::transport::streamable_http_server::session::ServerSseMessage;

    use super::*;
    use crate::session_stream::SessionStream;
    use crate::testing::stream_of;

    const IDLE: Duration = Duration::from_secs(50);
    const INTERVAL: Duration = Duration::from_secs(20);
    const TIMEOUT: Duration = Duration::from_secs(5);

    fn pings() -> Pings {
        Pings::new(Some(INTERVAL), TIMEOUT, Some(Arc::from("/mcp")))
    }

    /// A client and its session's watch, started now.
    fn watched() -> (Arc<SessionClient>, CancellationToken) {
        let end = CancellationToken::new();
        let client = Arc::new(SessionClient::new(end.clone()));
        tokio::spawn(Arc::clone(&client).watch(IDLE, pings()));
        (client, end)
    }

    /// Sleep until `after` the start, and say whether the session lives.
    async fn lives_at(start: Instant, after: Duration, end: &CancellationToken) -> bool {
        tokio::time::sleep_until(start + after).await;
        tokio::task::yield_now().await;
        !end.is_cancelled()
    }

    /// A client listening on a stream the transport never reads, so no ping
    /// reaches it, loses its session at the interval and the timeout of its
    /// last message, said once, and the stream is let go of.
    #[tokio::test(start_paused = true)]
    async fn a_listener_no_ping_reaches_is_given_up_at_the_interval_and_the_timeout() {
        let logs = nest_rs_testing::LogCapture::install();
        let start = Instant::now();
        let (client, end) = watched();
        let (sent, unread) = tokio::sync::mpsc::channel::<ServerSseMessage>(1);
        let _held =
            SessionStream::new(stream_of(unread), Some(Arc::clone(&client)), &pings(), true);

        let silence = INTERVAL + TIMEOUT;
        assert!(lives_at(start, silence - Duration::from_millis(1), &end).await);
        assert!(!lives_at(start, silence, &end).await, "ended at the bound");
        assert!(sent.is_closed(), "and its stream let go of");
        logs.expect_one(
            crate::TARGET,
            "mcp session closed: the client sent nothing back within the ping timeout",
        );
    }

    /// The idle timeout is held while a call waits on its answer, and counts
    /// again, whole, from that answer.
    #[tokio::test(start_paused = true)]
    async fn a_calls_wait_holds_the_idle_timeout_which_counts_again_from_its_answer() {
        let start = Instant::now();
        let (client, end) = watched();
        let (answer, waiting) = tokio::sync::mpsc::channel::<ServerSseMessage>(1);
        let mut call = SessionStream::new(
            stream_of(waiting),
            Some(Arc::clone(&client)),
            &pings(),
            false,
        );
        let waited = IDLE * 3;
        let answered = tokio::spawn(async move {
            let message = call.next().await;
            (Instant::now(), message.is_some())
        });

        assert!(lives_at(start, waited, &end).await, "held while it waits");
        answer
            .send(ServerSseMessage::retry(Duration::from_secs(3)))
            .await
            .expect("the call waits");
        let (at, carried) = answered.await.expect("the call is answered");
        assert!(carried);
        assert!(lives_at(at, IDLE - Duration::from_millis(1), &end).await);
        assert!(!lives_at(at, IDLE, &end).await, "idle from its answer");
    }

    /// What the server writes that no stream carries is no use: the session
    /// idles out at the idle timeout of its client's last message.
    #[tokio::test(start_paused = true)]
    async fn an_unused_session_idles_out_at_the_idle_timeout_of_its_clients_last_message() {
        let start = Instant::now();
        let (client, end) = watched();
        tokio::time::sleep(Duration::from_secs(10)).await;
        client.heard();
        client.used();

        assert!(
            lives_at(
                start,
                Duration::from_secs(10) + IDLE - Duration::from_millis(1),
                &end
            )
            .await
        );
        assert!(!lives_at(start, Duration::from_secs(10) + IDLE, &end).await);
    }
}
