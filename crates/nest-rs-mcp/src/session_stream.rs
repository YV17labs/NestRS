//! [`SessionStream`], a stream of a session as the transport holds it — what it
//! carries to the client is use of the session, a call's stream waiting on its
//! answer keeps the session in use, and the stream a client listens on (its
//! `GET`, first or resumed) asks a silent client MCP's `ping` — and
//! [`HeldStream`], what of it the session keeps to let go of when it ends.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures_util::Stream;
use futures_util::stream::BoxStream;
use futures_util::task::AtomicWaker;
use nest_rs_http::__private::{Due, KeepAlive};
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use tokio::time::Instant;

use crate::ping::{Pings, ping};
use crate::session_client::SessionClient;

/// What a session keeps of one of its streams: rmcp's stream itself, which the
/// session lets go of when it ends — so a write of its worker's to that
/// stream's channel, full while the transport cannot write, gives way — and
/// what the session's watch reads of it.
pub(crate) struct HeldStream {
    /// rmcp's stream, until the session lets go of it.
    inner: Mutex<Option<BoxStream<'static, ServerSseMessage>>>,
    /// The transport's task, woken when the session lets go.
    waker: AtomicWaker,
    /// The client listens on this stream: its `GET`, first or resumed; a
    /// call's stream, resumed or not, is none.
    listening: bool,
    /// The transport waits on this stream for a call's answer.
    awaited: AtomicBool,
    /// The session let go of it: the stream ends, asking nothing more.
    released: AtomicBool,
}

impl HeldStream {
    /// Whether the client listens on this stream.
    pub(crate) fn is_listening(&self) -> bool {
        self.listening
    }

    /// Whether a call's answer is awaited on this stream: the transport is
    /// waiting on it, as it does while the call runs.
    pub(crate) fn is_awaited(&self) -> bool {
        !self.listening && self.awaited.load(Ordering::Relaxed)
    }

    /// Let go of rmcp's stream: the transport reads its end next.
    pub(crate) fn release(&self) {
        self.released.store(true, Ordering::Relaxed);
        drop(
            self.inner
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        self.waker.wake();
    }
}

/// A stream of a session, which `client`'s session holds while the transport
/// does.
pub(crate) struct SessionStream {
    held: Arc<HeldStream>,
    /// `None` for a session the mount holds no client of, which nothing counts
    /// and nothing asks.
    client: Option<Arc<SessionClient>>,
    pings: Pings,
    /// Off on a stream the client does not listen on.
    keep_alive: KeepAlive,
    /// The last time the client was heard, as the keepalive was told it.
    told: Instant,
    ended: bool,
}

impl SessionStream {
    /// `inner`, a stream of `client`'s session — one it listens on, which asks
    /// it on the mount's `pings`, when `listening`.
    pub(crate) fn new<S>(
        inner: S,
        client: Option<Arc<SessionClient>>,
        pings: &Pings,
        listening: bool,
    ) -> Self
    where
        S: Stream<Item = ServerSseMessage> + Send + 'static,
    {
        let held = Arc::new(HeldStream {
            inner: Mutex::new(Some(Box::pin(inner))),
            waker: AtomicWaker::new(),
            listening,
            awaited: AtomicBool::new(false),
            released: AtomicBool::new(false),
        });
        if let Some(client) = &client {
            client.hold(Arc::clone(&held));
        }
        let interval = client.as_ref().and(pings.interval()).filter(|_| listening);
        Self {
            held,
            told: client
                .as_deref()
                .map_or_else(Instant::now, SessionClient::heard_at),
            client,
            pings: pings.clone(),
            keep_alive: KeepAlive::new(interval, pings.timeout()),
            ended: false,
        }
    }

    /// What the transport reads next of rmcp's stream, none once the session
    /// let go of it.
    fn poll_inner(&self, cx: &mut Context<'_>) -> Poll<Option<ServerSseMessage>> {
        self.held.waker.register(cx.waker());
        match self
            .held
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            Some(inner) => inner.as_mut().poll_next(cx),
            None => Poll::Ready(None),
        }
    }
}

impl Stream for SessionStream {
    type Item = ServerSseMessage;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<ServerSseMessage>> {
        let this = self.get_mut();
        if this.ended || this.held.released.load(Ordering::Relaxed) {
            this.ended = true;
            return Poll::Ready(None);
        }
        if let Some(client) = this.client.as_ref().filter(|_| this.held.listening) {
            let heard = client.heard_at();
            if heard > this.told {
                this.told = heard;
                this.keep_alive.heard_at(heard);
            }
            match this.keep_alive.poll_due(cx) {
                Poll::Ready(Due::Ping) => return Poll::Ready(Some(ping())),
                Poll::Ready(Due::Silent) => {
                    client.give_up(&this.pings);
                    this.ended = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {}
            }
        }
        let polled = this.poll_inner(cx);
        let waited = !this.held.listening
            && this
                .held
                .awaited
                .swap(polled.is_pending(), Ordering::Relaxed);
        // A message carried to the client, or a call's wait for its answer
        // over: either is use of the session.
        if (matches!(polled, Poll::Ready(Some(_))) || (waited && polled.is_ready()))
            && let Some(client) = &this.client
        {
            client.used();
        }
        if let Poll::Ready(None) = polled {
            this.ended = true;
        }
        polled
    }
}

impl Drop for SessionStream {
    fn drop(&mut self) {
        if let Some(client) = &self.client {
            if self.held.is_awaited() {
                client.used();
            }
            client.let_go(&self.held);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::StreamExt;
    use rmcp::model::ClientJsonRpcMessage;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::ping::is_ping_answer;

    const INTERVAL: Duration = Duration::from_secs(20);
    const TIMEOUT: Duration = Duration::from_secs(5);

    fn pings() -> Pings {
        Pings::new(Some(INTERVAL), TIMEOUT, Some(Arc::from("/mcp")))
    }

    fn answer(ping: &ServerSseMessage) -> ClientJsonRpcMessage {
        let sent =
            serde_json::to_value(ping.message.as_deref().expect("a message")).expect("serializes");
        serde_json::from_value(
            serde_json::json!({ "jsonrpc": "2.0", "id": sent["id"], "result": {} }),
        )
        .expect("an answer")
    }

    /// A stream whose session writes nothing: what a quiet session holds.
    fn quiet() -> futures_util::stream::Pending<ServerSseMessage> {
        futures_util::stream::pending()
    }

    fn listening(client: &Arc<SessionClient>) -> SessionStream {
        SessionStream::new(quiet(), Some(Arc::clone(client)), &pings(), true)
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_client_is_pinged_then_its_session_ended_once() {
        let start = Instant::now();
        let end = CancellationToken::new();
        let client = Arc::new(SessionClient::new(end.clone()));
        let mut stream = listening(&client);
        let mut other = listening(&client);

        let ping = stream.next().await.expect("a ping");
        assert_eq!(Instant::now() - start, INTERVAL);
        assert!(is_ping_answer(&answer(&ping)), "its answer is recognised");
        assert!(!end.is_cancelled());

        assert!(stream.next().await.is_none(), "given up, the stream ends");
        assert_eq!(Instant::now() - start, INTERVAL + TIMEOUT);
        assert!(end.is_cancelled(), "and the session with it");
        assert!(stream.next().await.is_none(), "and stays ended");

        let logs = nest_rs_testing::LogCapture::install();
        assert!(
            other.next().await.is_none(),
            "the session let go of every stream it held"
        );
        logs.expect_none(
            crate::TARGET,
            "mcp session closed: the client sent nothing back within the ping timeout",
        );
    }

    /// The client heard on another task, read when the wait ends, answers the
    /// ping: the next one comes an interval after it spoke.
    #[tokio::test(start_paused = true)]
    async fn a_client_heard_during_the_wait_is_pinged_an_interval_after_it_spoke() {
        let start = Instant::now();
        let end = CancellationToken::new();
        let client = Arc::new(SessionClient::new(end.clone()));
        let mut stream = listening(&client);

        let first = stream.next().await.expect("a ping");
        tokio::time::sleep(Duration::from_secs(1)).await;
        client.heard();
        let second = stream.next().await.expect("pinged again");

        assert_eq!(
            Instant::now() - start,
            INTERVAL + Duration::from_secs(1) + INTERVAL
        );
        assert!(!end.is_cancelled());
        let ids = [first, second].map(|ping| {
            serde_json::to_value(ping.message.as_deref().expect("a message")).expect("serializes")
                ["id"]
                .clone()
        });
        assert_ne!(ids[0], ids[1], "each ping takes an id of its own");
    }

    /// What the session writes passes through, the ping going first once due.
    #[tokio::test(start_paused = true)]
    async fn the_sessions_own_messages_pass_through() {
        let client = Arc::new(SessionClient::new(CancellationToken::new()));
        let written = ServerSseMessage::retry(Duration::from_secs(3));
        let mut stream = SessionStream::new(
            futures_util::stream::iter([written.clone()]).chain(quiet()),
            Some(client),
            &pings(),
            true,
        );

        let first = stream.next().await.expect("the session's message");
        assert_eq!(first.retry, written.retry);
        assert!(first.message.is_none());
        let ping = stream.next().await.expect("then the ping");
        assert!(ping.message.is_some());
    }

    /// A stream of a session the mount holds no client of asks nothing, and
    /// neither does one its client does not listen on.
    #[tokio::test(start_paused = true)]
    async fn a_stream_no_client_listens_on_asks_nothing() {
        let client = Arc::new(SessionClient::new(CancellationToken::new()));
        for mut stream in [
            SessionStream::new(quiet(), None, &pings(), true),
            SessionStream::new(quiet(), Some(client), &pings(), false),
        ] {
            tokio::select! {
                biased;
                asked = stream.next() => panic!("asked {asked:?}"),
                () = tokio::time::sleep((INTERVAL + TIMEOUT) * 3) => {}
            }
        }
    }

    /// A session's end lets go of its streams, whether or not the transport
    /// was reading them: the next read ends, and the session holds none.
    #[tokio::test]
    async fn a_closed_session_lets_go_of_its_streams() {
        let client = Arc::new(SessionClient::new(CancellationToken::new()));
        let (sent, unread) = tokio::sync::mpsc::channel::<ServerSseMessage>(1);
        let mut stream = SessionStream::new(
            tokio_stream_of(unread),
            Some(Arc::clone(&client)),
            &pings(),
            false,
        );

        client.close();

        assert!(sent.is_closed(), "rmcp's stream is let go of, unread");
        assert!(stream.next().await.is_none());
        let late = SessionStream::new(quiet(), Some(client), &pings(), true);
        assert!(late.held.inner.lock().expect("unpoisoned").is_none());
    }

    /// `receiver` as a stream.
    fn tokio_stream_of(
        mut receiver: tokio::sync::mpsc::Receiver<ServerSseMessage>,
    ) -> impl Stream<Item = ServerSseMessage> + Send + 'static {
        futures_util::stream::poll_fn(move |cx| receiver.poll_recv(cx))
    }
}
