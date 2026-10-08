//! The graphql-ws half of the `/graphql` mount, and the per-item posture seam
//! `#[subscription]` expands into.
//!
//! A subscription goes through the same gate as every operation, decided once at
//! subscribe, so every item is filtered against the ability captured then
//! ([`keep_masked_item`]) and the socket carries a lifetime ceiling
//! ([`GraphqlConfig::max_connection`](crate::GraphqlConfig::max_connection)).

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use async_graphql::http::{ALL_WEBSOCKET_PROTOCOLS, ClientMessage, WebSocketProtocols, WsMessage};
use async_graphql::parser::types::{DocumentOperations, OperationType};
use async_graphql::{Data, Executor};
use async_graphql_poem::GraphQLProtocol;
use futures_util::stream::SplitSink;
use futures_util::{FutureExt, SinkExt, Stream, StreamExt};
use poem::web::websocket::{CloseCode, Message, WebSocket, WebSocketStream};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response, Result};

use nest_rs_core::{Container, Correlation};
use nest_rs_http::DetachedWork;
use tracing::Instrument;

use crate::config::GraphqlConfig;
use crate::context::OperationBridge;

/// The composed schema as a bare [`Executor`], as the mount serves it: a
/// graphql-ws driver ([`async_graphql::http::WebSocket`]) takes an executor,
/// not a URL, so a test subscriber needs this.
#[doc(hidden)]
pub fn compose_schema(container: Container, config: &GraphqlConfig) -> impl Executor + 'static {
    crate::redact::Redacted(crate::resolver::build_schema(
        container,
        config,
        &DetachedWork::new(),
    ))
}

/// Keep or drop one masked subscription item, called per item by the
/// `#[subscription]` expansion: a refused row is dropped at `debug`; a mask
/// failure is dropped at `warn`, since an item has no error channel.
#[doc(hidden)]
pub fn keep_masked_item<T>(
    operation: &'static str,
    masked: Result<Option<T>, async_graphql::Error>,
) -> Option<T> {
    match masked {
        Ok(Some(item)) => Some(item),
        Ok(None) => {
            tracing::debug!(
                target: crate::TARGET,
                operation,
                reason = "not_granted",
                "subscription item withheld",
            );
            None
        }
        Err(err) => {
            tracing::warn!(
                target: crate::TARGET,
                operation,
                reason = "mask_failed",
                error = %err.message,
                "subscription item withheld",
            );
            None
        }
    }
}

/// The data handle a socket runs on, taken from the **upgrade request**: poem
/// answers the `101` before the connection task runs.
///
/// Always **non-transactional**: the request transaction is finalized with the
/// `101`, and carrying it onto an hours-long socket would pin a pooled connection
/// behind a transaction nobody commits.
fn socket_executor() -> Option<Arc<dyn nest_rs_database::Executor>> {
    nest_rs_database::current_executor()
        .map(|executor| executor.non_transactional().unwrap_or(executor))
}

/// The identity a socket runs under — id and actor — taken from the **upgrade
/// request**, as [`socket_executor`] is; nothing on the socket re-authenticates.
fn socket_correlation() -> Correlation {
    Correlation::inherited()
}

/// Why the server is ending a socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// The transport received the shutdown signal.
    GoingAway,
    /// The socket-lifetime ceiling elapsed.
    LifetimeReached,
}

impl Ending {
    /// The reason the Close frame carries — what the peer is to do about it.
    fn reason(self) -> &'static str {
        match self {
            Self::GoingAway => GOING_AWAY,
            Self::LifetimeReached => LIFETIME_REACHED,
        }
    }
}

/// RFC 6455 §7.4.1 **1001 Going Away**, its code for "a server going down".
const GOING_AWAY: &str = "the server is going away, reconnect to continue";

/// §7.4.1 **1001 Going Away** too, asking for a fresh upgrade that re-runs the
/// guard; the sentence a WebSocket gateway closes with.
const LIFETIME_REACHED: &str = "connection lifetime reached, re-upgrade to continue";

/// §7.4.1 **1011 Internal Error** — a subscription unwound, an unexpected
/// condition that prevented the server from serving the rest of the socket.
const UNWOUND: &str = "the subscription failed on the server";

/// The end the socket's server half arms: the shutdown signal, or the lifetime
/// ceiling (`None` ⇒ none), whichever comes first. The deadline is **absolute**,
/// not an idle timeout, so traffic never pushes it out.
async fn ends_at(going_away: impl Future<Output = ()>, ttl: Option<Duration>) -> Ending {
    let ceiling = async {
        match ttl {
            Some(ttl) => tokio::time::sleep(ttl).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        () = going_away => {
            // `debug`: shutdown is logged once; a line per socket is noise.
            tracing::debug!(
                target: crate::TARGET,
                close_code = u16::from(CloseCode::Away),
                "closing subscription socket: the server is going away",
            );
            Ending::GoingAway
        }
        () = ceiling => {
            tracing::info!(
                target: crate::TARGET,
                max_connection_secs = ttl.map_or(0, |ttl| ttl.as_secs()),
                close_code = u16::from(CloseCode::Away),
                "closing subscription socket: max lifetime reached",
            );
            Ending::LifetimeReached
        }
    }
}

/// How a socket ended, for its line.
enum Ended {
    /// Its client closed it, or the connection went.
    ByPeer,
    /// async-graphql closed it for a protocol fault — an unacknowledged
    /// connection, a malformed message, a repeated `connection_init`.
    Refused,
    /// The server ended it: its subscriptions completed, then 1001.
    ByServer,
    /// A subscription unwound; the socket closed with 1011.
    Unwound(Box<dyn std::any::Any + Send>),
}

/// The graphql-ws endpoint, reached through the GET dispatcher on the POST
/// endpoint's path.
pub(crate) struct SubscriptionEndpoint<E> {
    executor: E,
    bridge: Arc<OperationBridge>,
    max_connection: Option<Duration>,
    /// Every socket this mount serves: poem stops tracking a connection at its
    /// upgrade, so the shutdown window waits on this instead.
    sockets: DetachedWork,
}

impl<E> SubscriptionEndpoint<E> {
    pub(crate) fn new(
        executor: E,
        bridge: Arc<OperationBridge>,
        max_connection: Option<Duration>,
        sockets: DetachedWork,
    ) -> Self {
        Self {
            executor,
            bridge,
            max_connection,
            sockets,
        }
    }
}

impl<E: Executor> Endpoint for SubscriptionEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let (mut req, mut body) = req.split();
        // A denial at the upgrade is an ordinary HTTP response: no socket opens.
        if let Some(guard) = &self.bridge.op_guard
            && let Err(resp) = guard.before(&mut req).await
        {
            return Ok(resp);
        }
        let websocket = WebSocket::from_request(&req, &mut body).await?;
        let protocol = GraphQLProtocol::from_request(&req, &mut body).await?;
        let data = self.bridge.connection_data(&req);
        let executor = self.executor.clone();
        let max_connection = self.max_connection;
        let socket_executor = socket_executor();
        // `around` wraps the **whole socket**: one upgrade, one principal.
        let guard = self.bridge.op_guard.clone();
        // Read on the request task, before poem answers the `101`.
        let correlation = socket_correlation();
        let sockets = self.sockets.clone();

        Ok(websocket
            .protocols(ALL_WEBSOCKET_PROTOCOLS)
            .on_upgrade(move |stream| async move {
                // One span per socket: async-graphql's engine hides operation
                // boundaries from this crate.
                let span = nest_rs_core::operation_span!(crate::unit::SUBSCRIPTION, &correlation,);
                let line = SubscriptionLine::new(correlation.clone(), span.clone());
                let end = ends_at(sockets.going_away(), max_connection);
                // A local slot, since the guard scopes a `()` future.
                let mut ended: Option<Ended> = None;
                let serving: crate::BoxFuture<'_, ()> = Box::pin(async {
                    let served = serve_socket(stream, executor, protocol.0, data, end);
                    ended = Some(match max_connection {
                        // A peer that stopped reading cannot hold the socket past
                        // the ceiling plus the close grace.
                        Some(ttl) => tokio::time::timeout(
                            ttl.saturating_add(DetachedWork::CLOSE_GRACE),
                            served,
                        )
                        .await
                        .unwrap_or_else(|_| {
                            tracing::debug!(
                                target: crate::TARGET,
                                close_grace_ms = u64::try_from(
                                    DetachedWork::CLOSE_GRACE.as_millis()
                                )
                                .unwrap_or(u64::MAX),
                                "subscription socket dropped: it did not take its close \
                                 within the grace",
                            );
                            Ended::ByServer
                        }),
                        None => served.await,
                    });
                });
                let guarded: crate::BoxFuture<'_, ()> = match &guard {
                    Some(guard) => guard.around(&req, serving),
                    None => serving,
                };
                // Executor outermost, ability inside, as on the POST path.
                let served: crate::BoxFuture<'_, ()> = match socket_executor {
                    Some(executor) => {
                        Box::pin(nest_rs_database::with_request_executor(executor, guarded))
                    }
                    None => guarded,
                };
                // Dropped at the window's close, which `line`'s `Drop` files `cancelled`.
                let _ = sockets
                    .run(nest_rs_core::with_request_scope(None, correlation, served))
                    .instrument(span)
                    .await;
                if let Some(ended) = ended {
                    line.settle(ended);
                }
            })
            .into_response())
    }
}

/// Serve one graphql-ws socket until its peer ends it, the protocol refuses
/// it, or `end` resolves — then complete every running subscription, answer
/// what is still answering, and close with 1001, a seam async-graphql's loop
/// lacks.
///
/// At `end`, [`Inbound`] hands the engine a `stop` per running subscription,
/// so the engine writes each `complete` in the negotiated protocol's wording.
async fn serve_socket<E: Executor>(
    stream: WebSocketStream,
    executor: E,
    protocol: WebSocketProtocols,
    data: Data,
    end: impl Future<Output = Ending> + Send + 'static,
) -> Ended {
    let (mut sink, socket) = stream.split();
    let started: Started = Arc::default();
    let ending: Arc<OnceLock<Ending>> = Arc::default();
    let inbound = Inbound {
        socket,
        end: Box::pin(end),
        started: Arc::clone(&started),
        ending: Arc::clone(&ending),
        stopping: None,
    };
    let mut engine =
        async_graphql::http::WebSocket::from_message_stream(executor, inbound, protocol)
            .connection_data(data);
    loop {
        // A subscription's stream is developer code polled here: a panic closes
        // the socket with 1011.
        let next = match AssertUnwindSafe(engine.next()).catch_unwind().await {
            Ok(next) => next,
            Err(payload) => {
                close_socket(&mut sink, CloseCode::Error, UNWOUND).await;
                return Ended::Unwound(payload);
            }
        };
        match next {
            Some(WsMessage::Text(text)) => {
                if let Some(id) = completed_id(&text) {
                    lock(&started).remove(&id);
                }
                if sink.send(Message::Text(text)).await.is_err() {
                    return Ended::ByPeer;
                }
            }
            Some(WsMessage::Close(code, reason)) => {
                close_socket(&mut sink, CloseCode::from(code), &reason).await;
                return Ended::Refused;
            }
            None => break,
        }
    }
    match ending.get() {
        Some(ending) => {
            close_socket(&mut sink, CloseCode::Away, ending.reason()).await;
            Ended::ByServer
        }
        None => {
            // The flush puts the protocol layer's queued Close echo on the wire.
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the peer already closed; the flush only echoes its Close"
            )]
            let _ = SinkExt::close(&mut sink).await;
            Ended::ByPeer
        }
    }
}

/// The operations a peer started and nobody has finished, by id.
type Started = Arc<Mutex<HashMap<String, Operation>>>;

fn lock(started: &Started) -> std::sync::MutexGuard<'_, HashMap<String, Operation>> {
    started
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How the socket's end treats an operation sent over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    /// A subscription has no end of its own, so the socket's end completes it.
    Subscription,
    /// A query or a mutation, answered first.
    Answer,
}

impl Operation {
    /// Read off the request's document, parsed once (async-graphql caches it).
    /// One that does not parse is answered at once, so it is an [`Operation::Answer`].
    fn of(request: &mut async_graphql::Request) -> Self {
        let selected = request.operation_name.clone();
        let Ok(document) = request.parsed_query() else {
            return Self::Answer;
        };
        let ty = match &document.operations {
            DocumentOperations::Single(operation) => Some(operation.node.ty),
            DocumentOperations::Multiple(operations) => selected.as_deref().and_then(|name| {
                operations
                    .iter()
                    .find(|(candidate, _)| candidate.as_str() == name)
                    .map(|(_, operation)| operation.node.ty)
            }),
        };
        if ty == Some(OperationType::Subscription) {
            Self::Subscription
        } else {
            Self::Answer
        }
    }
}

/// The id of the operation a server frame completes, when it is a `complete`.
///
/// Read off the frame's opening rather than by parsing every `next`: serde
/// writes the tag first, pinned by a test against async-graphql's output.
/// Missing one only leaves an id in [`Started`], whose `stop` the engine ignores.
fn completed_id(frame: &str) -> Option<String> {
    if !frame.starts_with(r#"{"type":"complete""#) {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(frame)
        .ok()?
        .get("id")?
    {
        serde_json::Value::String(id) => Some(id.clone()),
        _ => None,
    }
}

/// The peer's half of a socket, as the protocol engine reads it, tracking which
/// operations run; once `end` resolves it yields a `stop` per running
/// subscription instead, and ends once nothing is left answering.
struct Inbound<S> {
    socket: S,
    end: std::pin::Pin<Box<dyn Future<Output = Ending> + Send>>,
    started: Started,
    ending: Arc<OnceLock<Ending>>,
    /// The `stop`s left to hand the engine, once `end` has resolved.
    stopping: Option<std::vec::IntoIter<String>>,
}

impl<S> Stream for Inbound<S>
where
    S: Stream<Item = std::io::Result<Message>> + Unpin,
{
    type Item = serde_json::Result<ClientMessage>;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.stopping.is_none()
            && let Poll::Ready(ending) = this.end.as_mut().poll(cx)
        {
            #[expect(clippy::let_underscore_must_use, reason = "the first ending stands")]
            let _ = this.ending.set(ending);
            let mut started = lock(&this.started);
            let subscriptions: Vec<String> = started
                .iter()
                .filter(|(_, operation)| **operation == Operation::Subscription)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &subscriptions {
                started.remove(id);
            }
            drop(started);
            this.stopping = Some(subscriptions.into_iter());
        }
        if let Some(stopping) = this.stopping.as_mut() {
            if let Some(id) = stopping.next() {
                return Poll::Ready(Some(Ok(ClientMessage::Stop { id })));
            }
            // The engine wakes on what is still answering, and asks again.
            return if lock(&this.started).is_empty() {
                Poll::Ready(None)
            } else {
                Poll::Pending
            };
        }
        loop {
            match std::pin::Pin::new(&mut this.socket).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None | Some(Err(_))) => return Poll::Ready(None),
                Poll::Ready(Some(Ok(message))) => {
                    if !(message.is_text() || message.is_binary()) {
                        continue;
                    }
                    let mut parsed = serde_json::from_slice::<ClientMessage>(&message.into_bytes());
                    match &mut parsed {
                        Ok(ClientMessage::Start { id, payload }) => {
                            let operation = Operation::of(payload);
                            lock(&this.started).insert(id.clone(), operation);
                        }
                        Ok(ClientMessage::Stop { id }) => {
                            lock(&this.started).remove(id);
                        }
                        _ => {}
                    }
                    return Poll::Ready(Some(parsed));
                }
            }
        }
    }
}

/// Put a Close frame on the wire, then flush — best-effort, since a peer that
/// has already gone cannot be told anything.
async fn close_socket(
    sink: &mut SplitSink<WebSocketStream, Message>,
    code: CloseCode,
    reason: &str,
) {
    if let Err(err) = sink
        .send(Message::Close(Some((code, reason.to_owned()))))
        .await
    {
        tracing::debug!(
            target: crate::TARGET,
            error = %nest_rs_core::error_message(&err),
            close_code = u16::from(code),
            "subscription socket close frame undelivered",
        );
        return;
    }
    #[expect(
        clippy::let_underscore_must_use,
        reason = "our Close is on the wire; a peer gone before the flush has nothing left to be told"
    )]
    let _ = SinkExt::close(sink).await;
}

/// Files the subscription's line **on drop**, so an aborted socket files
/// `cancelled` (or `panic`); one that finishes says how through
/// [`settle`](Self::settle).
///
/// The correlation is held: a `Drop` runs while the future is torn down, not
/// reliably inside the scope that future installed.
struct SubscriptionLine {
    correlation: nest_rs_core::Correlation,
    /// The subscription's span, which the outcome is recorded on for the export.
    span: tracing::Span,
    started: std::time::Instant,
    outcome: Option<&'static str>,
}

impl SubscriptionLine {
    fn new(correlation: nest_rs_core::Correlation, span: tracing::Span) -> Self {
        Self {
            correlation,
            span,
            started: std::time::Instant::now(),
            outcome: None,
        }
    }

    /// Say how the socket ended: `ok` by its peer, `error` refused by the
    /// protocol, `cancelled` by the server, `panic` when a subscription unwound.
    fn settle(mut self, ended: Ended) {
        use nest_rs_core::operation_log::{CANCELLED, ERROR, OK, PANIC};
        self.outcome = Some(match ended {
            Ended::ByPeer => OK,
            Ended::Refused => ERROR,
            Ended::ByServer => CANCELLED,
            Ended::Unwound(payload) => {
                nest_rs_core::RequestContinuation::new(None, self.correlation.clone()).enter(
                    || {
                        nest_rs_core::contained_panic!(
                            target: crate::TARGET,
                            &*payload,
                            "graphql subscription panicked; the socket is closed with an internal \
                             error",
                            close_code = u16::from(CloseCode::Error),
                        );
                    },
                );
                PANIC
            }
        });
    }
}

impl Drop for SubscriptionLine {
    fn drop(&mut self) {
        let outcome = self.outcome.unwrap_or(if std::thread::panicking() {
            nest_rs_core::operation_log::PANIC
        } else {
            nest_rs_core::operation_log::CANCELLED
        });
        nest_rs_core::RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            nest_rs_core::operation_line!(
                crate::unit::SUBSCRIPTION,
                span: &self.span,
                outcome: outcome,
                started: self.started,
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::Arc;

    use super::*;

    /// A panic inside a `Drop` during an unwind aborts the process, and this
    /// `Drop` reaches a `tokio` task-local.
    #[test]
    fn the_subscription_line_fires_once_and_cannot_panic_off_a_runtime() {
        let logs = nest_rs_testing::LogCapture::install();

        // No `#[tokio::test]`: at teardown there may be no runtime left.
        drop(SubscriptionLine::new(
            nest_rs_core::Correlation::minted(None),
            tracing::Span::none(),
        ));

        let served = logs.find(
            nest_rs_core::operation_log::TARGET,
            crate::unit::SUBSCRIPTION.name(),
        );
        assert_eq!(
            served.len(),
            1,
            "exactly one line per connection: {served:?}"
        );
        assert_eq!(
            served[0].field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::CANCELLED),
        );
        assert!(served[0].field("duration_ms").is_some());
    }

    #[test]
    fn a_settled_socket_files_the_end_it_saw() {
        use nest_rs_core::operation_log::{CANCELLED, ERROR, OK};
        for (ended, outcome) in [
            (Ended::ByPeer, OK),
            (Ended::Refused, ERROR),
            (Ended::ByServer, CANCELLED),
        ] {
            let logs = nest_rs_testing::LogCapture::install();
            SubscriptionLine::new(
                nest_rs_core::Correlation::minted(None),
                tracing::Span::none(),
            )
            .settle(ended);
            let served = logs.find(
                nest_rs_core::operation_log::TARGET,
                crate::unit::SUBSCRIPTION.name(),
            );
            assert_eq!(served.len(), 1, "{served:?}");
            assert_eq!(served[0].field("outcome").as_deref(), Some(outcome));
        }
    }

    #[tokio::test]
    async fn the_subscription_line_fires_once_inside_a_runtime() {
        let logs = nest_rs_testing::LogCapture::install();
        let correlation = nest_rs_core::Correlation::minted(None);

        nest_rs_core::with_request_scope(None, correlation.clone(), async {
            let _line = SubscriptionLine::new(correlation, tracing::Span::none());
        })
        .await;

        assert_eq!(
            logs.find(
                nest_rs_core::operation_log::TARGET,
                crate::unit::SUBSCRIPTION.name()
            )
            .len(),
            1,
        );
    }

    /// A request's lazy transaction handle, stepping out onto the pool.
    struct Transaction;
    /// The pool handle it steps out onto.
    struct Pool;

    impl nest_rs_database::Executor for Transaction {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
            Some(Arc::new(Pool))
        }
    }

    impl nest_rs_database::Executor for Pool {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[tokio::test]
    async fn a_socket_never_carries_the_upgrades_transaction() {
        let captured = nest_rs_database::with_request_executor(Arc::new(Transaction), async {
            socket_executor()
        })
        .await;

        let captured = captured.expect("the upgrade runs inside the request boundary");
        assert!(
            captured.as_any().is::<Pool>(),
            "the socket runs on the pool, not on the request transaction",
        );
    }

    #[tokio::test]
    async fn a_socket_runs_in_the_upgrades_trace() {
        let upgrade = Correlation::minted(None);
        let captured =
            nest_rs_core::with_request_scope(None, upgrade.clone(), async { socket_correlation() })
                .await;

        assert_eq!(captured.trace_id(), upgrade.trace_id());
    }

    #[tokio::test]
    async fn a_socket_runs_under_the_upgrades_actor() {
        let captured = nest_rs_core::with_request_scope(None, Correlation::minted(None), async {
            nest_rs_core::set_actor_id("alice-42");
            socket_correlation()
        })
        .await;

        assert_eq!(captured.actor_id(), Some("alice-42"));
    }

    #[tokio::test]
    async fn a_socket_with_no_upgrade_to_inherit_from_starts_its_own_trace() {
        assert!(nest_rs_core::current_trace_id().is_none());
        assert!(socket_correlation().actor_id().is_none());
    }

    #[tokio::test]
    async fn a_socket_keeps_a_handle_that_is_already_the_pool() {
        let captured =
            nest_rs_database::with_request_executor(Arc::new(Pool), async { socket_executor() })
                .await;

        assert!(
            captured.is_some_and(|executor| executor.as_any().is::<Pool>()),
            "an upgrade on a safe method already holds the pool — keep it",
        );
    }

    #[tokio::test]
    async fn a_socket_without_an_orm_carries_nothing() {
        assert!(socket_executor().is_none());
    }

    /// Pins the opening [`completed_id`] reads against async-graphql's own
    /// `complete` frame, in both negotiated protocols.
    #[tokio::test]
    async fn the_complete_async_graphql_writes_is_the_one_read() {
        use async_graphql::futures_util::stream;
        use async_graphql::{EmptyMutation, Object, Schema, Subscription};

        struct Query;
        #[Object]
        impl Query {
            async fn up(&self) -> bool {
                true
            }
        }
        struct Once;
        #[Subscription]
        impl Once {
            async fn once(&self) -> impl Stream<Item = i32> {
                stream::iter([1])
            }
        }

        for (protocol, subscribe) in [
            (WebSocketProtocols::GraphQLWS, "subscribe"),
            (WebSocketProtocols::SubscriptionsTransportWS, "start"),
        ] {
            let schema = Schema::new(Query, EmptyMutation, Once);
            let inbound = stream::iter([
                r#"{"type":"connection_init"}"#.to_owned(),
                format!(r#"{{"id":"7","type":"{subscribe}","payload":{{"query":"subscription {{ once }}"}}}}"#),
            ])
            .chain(stream::pending());
            let mut engine = async_graphql::http::WebSocket::new(schema, inbound, protocol);
            let completed = loop {
                match tokio::time::timeout(Duration::from_secs(5), engine.next()).await {
                    Ok(Some(WsMessage::Text(frame))) => {
                        if let Some(id) = completed_id(&frame) {
                            break id;
                        }
                    }
                    other => panic!("{protocol:?}: no complete before {other:?}"),
                }
            };
            assert_eq!(completed, "7", "{protocol:?}");
        }
    }

    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test waits for the end; how it ended is asserted elsewhere"
    )]
    async fn at_its_end_the_peers_half_stops_every_running_operation_and_ends() {
        use async_graphql::futures_util::stream;

        let frames = stream::iter([
            Ok(Message::Text(
                r#"{"id":"a","type":"subscribe","payload":{"query":"subscription { x }"}}"#.into(),
            )),
            Ok(Message::Text(
                r#"{"id":"b","type":"subscribe","payload":{"query":"subscription { x }"}}"#.into(),
            )),
            Ok(Message::Text(r#"{"id":"a","type":"complete"}"#.into())),
        ])
        .chain(stream::pending());
        let (end_now, ended) = tokio::sync::oneshot::channel::<()>();
        let ending: Arc<OnceLock<Ending>> = Arc::default();
        let mut inbound = Inbound {
            socket: Box::pin(frames),
            end: Box::pin(async move {
                let _ = ended.await;
                Ending::GoingAway
            }),
            started: Started::default(),
            ending: Arc::clone(&ending),
            stopping: None,
        };
        for _ in 0..3 {
            assert!(matches!(inbound.next().await, Some(Ok(_))));
        }
        end_now.send(()).expect("the end is armed");

        let stopped: Vec<String> = inbound
            .map(|message| match message {
                Ok(ClientMessage::Stop { id }) => id,
                _ => panic!("only stops follow the end"),
            })
            .collect()
            .await;
        assert_eq!(
            stopped,
            ["b"],
            "the one the peer started and nobody finished"
        );
        assert_eq!(ending.get(), Some(&Ending::GoingAway));
    }

    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test waits for the end; how it ended is asserted elsewhere"
    )]
    async fn at_its_end_an_operation_still_answering_is_left_to_finish() {
        use async_graphql::futures_util::stream;

        let frames = stream::iter([
            Ok(Message::Text(
                r#"{"id":"s","type":"subscribe","payload":{"query":"subscription { x }"}}"#.into(),
            )),
            Ok(Message::Text(
                r#"{"id":"q","type":"subscribe","payload":{"query":"mutation { y }"}}"#.into(),
            )),
        ])
        .chain(stream::pending());
        let started = Started::default();
        let (end_now, ended) = tokio::sync::oneshot::channel::<()>();
        let mut inbound = Inbound {
            socket: Box::pin(frames),
            end: Box::pin(async move {
                let _ = ended.await;
                Ending::GoingAway
            }),
            started: Arc::clone(&started),
            ending: Arc::default(),
            stopping: None,
        };
        for _ in 0..2 {
            assert!(matches!(inbound.next().await, Some(Ok(_))));
        }
        end_now.send(()).expect("the end is armed");

        assert!(matches!(
            inbound.next().await,
            Some(Ok(ClientMessage::Stop { id })) if id == "s"
        ));
        assert!(
            async_graphql::futures_util::poll!(inbound.next()).is_pending(),
            "the mutation is still answering",
        );
        lock(&started).remove("q");
        assert!(
            inbound.next().await.is_none(),
            "nothing left answering, so the peer's half ends"
        );
    }
}
