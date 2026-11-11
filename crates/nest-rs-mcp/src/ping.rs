//! [`PingingStream`], a session's stream asking its silent client MCP's `ping`
//! (the specification's *Ping* utility), and [`SessionClient`], what the mount
//! knows of that client: when it last spoke, and how its session ends.
//!
//! The ping is written on the stream itself, never sent through rmcp's
//! session worker: the worker re-arms its idle timer on every message either
//! way, and keeps one sent while no stream is open to replay later. Its answer
//! is taken out before the worker sees it ([`is_ping_answer`]), so
//! `session_idle_timeout` counts neither, and only a session holding a stream,
//! which a vanished client would hold open, is asked anything.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::Stream;
use nest_rs_http::__private::{Due, KeepAlive};
use rmcp::model::{
    ClientJsonRpcMessage, NumberOrString, PingRequest, ServerJsonRpcMessage, ServerRequest,
};
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// How every id the mount's pings take opens: rmcp numbers its own requests,
/// so no request of rmcp's, or of a host writing through its peer, takes one.
/// A random UUID follows, since a session restored from the app's store, or
/// served by another instance, gets a client of its own and no count to go on.
const PING_ID_PREFIX: &str = "ping-";

/// What one mount's sessions are pinged on, and the mount their line names.
#[derive(Clone, Debug)]
pub(crate) struct Pings {
    interval: Option<Duration>,
    timeout: Duration,
    path: Option<Arc<str>>,
}

impl Pings {
    /// A ping once a session's client is silent for `interval`, its session
    /// ended when nothing comes back within `timeout`, on the mount served at
    /// `path`; `interval` `None` pings no one.
    pub(crate) fn new(
        interval: Option<Duration>,
        timeout: Duration,
        path: Option<Arc<str>>,
    ) -> Self {
        Self {
            interval,
            timeout,
            path,
        }
    }
}

/// What the mount knows of one session's client.
#[derive(Debug)]
pub(crate) struct SessionClient {
    /// When the session opened: [`heard`](Self::heard) counts from it.
    opened: Instant,
    /// Nanoseconds from `opened` to the client's last message.
    heard: AtomicU64,
    /// Whether a stream found the client gone: the first one says it.
    gone: AtomicBool,
    /// Ends the session: its worker stops, its place frees, its streams end.
    end: CancellationToken,
}

impl SessionClient {
    /// A client heard as its session opens, which `end` ends.
    pub(crate) fn new(end: CancellationToken) -> Self {
        Self {
            opened: Instant::now(),
            heard: AtomicU64::new(0),
            gone: AtomicBool::new(false),
            end,
        }
    }

    /// A message of any kind came from the client: it is there.
    pub(crate) fn heard(&self) {
        let since = Instant::now().saturating_duration_since(self.opened);
        let nanos = u64::try_from(since.as_nanos()).unwrap_or(u64::MAX);
        self.heard.fetch_max(nanos, Ordering::Relaxed);
    }

    /// When the client's last message arrived.
    fn heard_at(&self) -> Instant {
        self.opened + Duration::from_nanos(self.heard.load(Ordering::Relaxed))
    }

    /// End the session of a client that answered nothing, said once for the
    /// mount `pings` names.
    fn give_up(&self, pings: &Pings) {
        if self.gone.swap(true, Ordering::Relaxed) {
            return;
        }
        tracing::warn!(
            target: crate::TARGET,
            host = crate::operation::REQUEST.handler().host_name(),
            path = pings.path.as_deref(),
            ping_timeout_ms = pings.timeout.as_millis(),
            "mcp session closed: the client sent nothing back within the ping timeout",
        );
        self.end.cancel();
    }
}

/// A ping, under an id no ping of its session took before, whichever client of
/// that session asks it.
fn ping() -> ServerSseMessage {
    let id = format!("{PING_ID_PREFIX}{}", uuid::Uuid::new_v4().simple());
    ServerSseMessage::from_message(ServerJsonRpcMessage::request(
        ServerRequest::PingRequest(PingRequest::default()),
        NumberOrString::String(Arc::from(id)),
    ))
}

/// Whether `message` answers one of the mount's pings — its result, or an
/// error from a client that refused it — which rmcp's worker never sees.
pub(crate) fn is_ping_answer(message: &ClientJsonRpcMessage) -> bool {
    let id = match message {
        ClientJsonRpcMessage::Response(response) => Some(&response.id),
        ClientJsonRpcMessage::Error(error) => error.id.as_ref(),
        _ => None,
    };
    matches!(id, Some(NumberOrString::String(id)) if id.starts_with(PING_ID_PREFIX))
}

/// A session's stream — its `GET`, or one resumed — asking its client a ping
/// once it is silent for the interval, ahead of whatever the stream holds, and
/// ending the session when nothing comes back within the timeout.
pub(crate) struct PingingStream<S> {
    inner: S,
    /// `None` for a session the mount holds no client of, which nothing asks.
    client: Option<Arc<SessionClient>>,
    pings: Pings,
    keep_alive: KeepAlive,
    /// The last time the client was heard, as the keepalive was told it.
    told: Instant,
    ended: bool,
}

impl<S> PingingStream<S> {
    /// `inner`, asking `client` on the mount's `pings`.
    pub(crate) fn new(inner: S, client: Option<Arc<SessionClient>>, pings: &Pings) -> Self {
        let interval = client.as_ref().and(pings.interval);
        Self {
            inner,
            told: client
                .as_deref()
                .map_or_else(Instant::now, SessionClient::heard_at),
            client,
            pings: pings.clone(),
            keep_alive: KeepAlive::new(interval, pings.timeout),
            ended: false,
        }
    }
}

impl<S> Stream for PingingStream<S>
where
    S: Stream<Item = ServerSseMessage> + Unpin,
{
    type Item = ServerSseMessage;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<ServerSseMessage>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        if let Some(client) = &this.client {
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
        Pin::new(&mut this.inner).poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;

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

    #[tokio::test(start_paused = true)]
    async fn a_silent_client_is_pinged_then_its_session_ended_once() {
        let start = Instant::now();
        let end = CancellationToken::new();
        let client = Arc::new(SessionClient::new(end.clone()));
        let mut stream = PingingStream::new(quiet(), Some(client.clone()), &pings());
        let mut other = PingingStream::new(quiet(), Some(client), &pings());

        let ping = stream.next().await.expect("a ping");
        assert_eq!(Instant::now() - start, INTERVAL);
        assert!(is_ping_answer(&answer(&ping)), "its answer is recognised");
        assert!(!end.is_cancelled());

        assert!(stream.next().await.is_none(), "given up, the stream ends");
        assert_eq!(Instant::now() - start, INTERVAL + TIMEOUT);
        assert!(end.is_cancelled(), "and the session with it");
        assert!(stream.next().await.is_none(), "and stays ended");

        let logs = nest_rs_testing::LogCapture::install();
        other.next().await.expect("the other stream's ping");
        assert!(other.next().await.is_none());
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
        let mut stream = PingingStream::new(quiet(), Some(client.clone()), &pings());

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

    /// A session restored from the app's store, or served by another instance,
    /// gets a client of its own, with no count to go on: its pings never take an
    /// id one asked before within the session.
    #[test]
    fn a_session_bound_again_never_asks_under_an_id_it_used() {
        let id = |ping: ServerSseMessage| {
            serde_json::to_value(ping.message.as_deref().expect("a message")).expect("serializes")
                ["id"]
                .clone()
        };

        let asked = id(ping());
        let asked_again = id(ping());

        assert_ne!(asked, asked_again, "one id, asked twice in one session");
        for asked in [asked, asked_again] {
            let answer: ClientJsonRpcMessage = serde_json::from_value(
                serde_json::json!({ "jsonrpc": "2.0", "id": asked, "result": {} }),
            )
            .expect("an answer");
            assert!(is_ping_answer(&answer), "{asked}");
        }
    }

    /// What the session writes passes through, the ping going first once due.
    #[tokio::test(start_paused = true)]
    async fn the_sessions_own_messages_pass_through() {
        let client = Arc::new(SessionClient::new(CancellationToken::new()));
        let written = ServerSseMessage::retry(Duration::from_secs(3));
        let mut stream = PingingStream::new(
            futures_util::stream::iter([written.clone()]).chain(quiet()),
            Some(client),
            &pings(),
        );

        let first = stream.next().await.expect("the session's message");
        assert_eq!(first.retry, written.retry);
        assert!(first.message.is_none());
        let ping = stream.next().await.expect("then the ping");
        assert!(ping.message.is_some());
    }

    /// A stream of a session the mount holds no client of asks nothing.
    #[tokio::test(start_paused = true)]
    async fn a_stream_with_no_client_asks_nothing() {
        let mut stream = PingingStream::new(quiet(), None, &pings());
        tokio::select! {
            biased;
            asked = stream.next() => panic!("asked {asked:?}"),
            () = tokio::time::sleep((INTERVAL + TIMEOUT) * 3) => {}
        }
    }

    #[test]
    fn only_an_answer_to_a_ping_of_the_mounts_is_one() {
        let message = |value: serde_json::Value| -> ClientJsonRpcMessage {
            serde_json::from_value(value).expect("a message")
        };
        assert!(is_ping_answer(&message(
            serde_json::json!({ "jsonrpc": "2.0", "id": "ping-3", "result": {} })
        )));
        assert!(is_ping_answer(&message(serde_json::json!({
            "jsonrpc": "2.0",
            "id": "ping-3",
            "error": { "code": -32601, "message": "Method not found" },
        }))));
        for other in [
            serde_json::json!({ "jsonrpc": "2.0", "id": 3, "result": {} }),
            serde_json::json!({ "jsonrpc": "2.0", "id": "elicit-3", "result": {} }),
            serde_json::json!({ "jsonrpc": "2.0", "id": "ping-3", "method": "ping" }),
            serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        ] {
            assert!(!is_ping_answer(&message(other.clone())), "{other}");
        }
    }
}
