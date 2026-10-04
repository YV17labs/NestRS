//! The graphql-ws half of the `/graphql` mount, and the per-item posture seam
//! `#[subscription]` expands into.
//!
//! A subscription is an operation, so it goes through the gate every other
//! operation goes through — the same [`GraphqlOperationGuard`](crate::GraphqlOperationGuard), the same
//! `#[authorize]`/`#[public]`. What differs is *when* it answers: once at
//! subscribe, then repeatedly, for as long as the socket lives. That produces
//! the two obligations this module carries and the POST path does not:
//!
//! - **the guard's decision is made once and must keep applying**, so every
//!   item is filtered against the ability captured at subscribe
//!   ([`keep_masked_item`], fed by `nest_rs_authz::graphql::masked_item_for`);
//! - **the socket must not outlive that decision**, so it carries the same
//!   lifetime ceiling a WebSocket gateway does
//!   ([`GraphqlConfig::max_connection`](crate::GraphqlConfig::max_connection)).

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

/// The composed schema as a bare [`Executor`] — the seam a *subscriber* needs.
///
/// Queries and mutations are testable through the mounted endpoint, because
/// `TestApp`'s client speaks the protocol they use. A subscription's protocol is
/// graphql-ws, and the thing that speaks it
/// ([`async_graphql::http::WebSocket`], and `GraphQLWebSocket` above it) takes
/// an executor rather than a URL — so a witness that boots the documented wiring
/// and then asserts *what a subscriber actually receives* has to reach the
/// executor the mount serves.
///
/// It hands back the schema opaquely: the discovered roots stay `pub(crate)`,
/// and the only thing a caller can do with the value is execute against it,
/// which is the whole point. And it is the mount's executor whole — the schema
/// behind [`Redacted`](crate::redact::Redacted), so a subscriber reads errors
/// as a client of the mount does.
#[doc(hidden)]
pub fn compose_schema(container: Container, config: &GraphqlConfig) -> impl Executor + 'static {
    // No transport carries what a subscriber drives by hand, so its batches are
    // carried by work nothing stops.
    crate::redact::Redacted(crate::resolver::build_schema(
        container,
        config,
        &DetachedWork::new(),
    ))
}

/// Keep or drop one masked subscription item.
///
/// Called per item by the `#[subscription]` expansion, so the three outcomes are
/// worded once rather than inlined into every operation:
///
/// - masked and allowed ⇒ push it;
/// - the subscriber's ability refuses the row ⇒ drop it. This is the steady
///   state of a stream two principals read differently, not an anomaly, so it
///   logs at `debug`;
/// - masking failed ⇒ drop it and say so at `warn`. A stream item has no error
///   channel of its own (the field's type is the item's, not a `Result`), so
///   failing closed *is* dropping it — and an item silently vanishing because a
///   wire value would not reconcile is exactly the thing an operator has to be
///   able to find.
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

/// The data handle a socket runs on, taken from the **upgrade request** — the
/// last moment ambient state exists.
///
/// poem answers the `101` before the connection task runs, so the request
/// boundary's executor is already gone by the time an operation on that socket
/// resolves. Without this a `Repo`-backed subscription fails on every item for
/// want of one, which is fail-closed but useless.
///
/// Always **non-transactional**, and that is the load-bearing half. A request
/// transaction belongs to the request that opened it and is finalized when the
/// `101` returns; carrying it onto a connection that may live for hours would
/// pin a pooled connection for that whole time and write through a transaction
/// nobody will ever commit. [`non_transactional`] steps out of it; a handle that
/// is already the pool has nothing to step out of and is kept. The atomicity
/// given up here is atomicity nothing on this path can use — a subscription
/// reads, and a mutation is not a subscription.
///
/// [`non_transactional`]: nest_rs_database::Executor::non_transactional
fn socket_executor() -> Option<Arc<dyn nest_rs_database::Executor>> {
    nest_rs_database::current_executor()
        .map(|executor| executor.non_transactional().unwrap_or(executor))
}

/// The identity a socket runs under, taken from the **upgrade request** — the
/// same last moment [`socket_executor`] reads, and for the same reason.
///
/// The upgrade *is* an HTTP request, so the socket inherits that request's id,
/// and its actor: the guard that just ran resolved one, and nothing on this
/// socket ever re-authenticates anybody. Without it every subscription item,
/// every withheld row and every mask failure below files under no request at
/// all — poem answers the `101` before the connection task runs, so the request
/// boundary is gone by the time the first item is produced.
///
/// A mint is the honest answer where there is nothing to inherit — an endpoint
/// mounted outside the HTTP edge. The socket's own events still group with each
/// other, which is strictly more than nothing.
///
/// **Identity, never the upgrade's resources.** The socket already steps out of
/// the request transaction ([`socket_executor`]) and holds no request scope, for
/// one reason: a connection that may live for hours would pin whatever the
/// upgrade resolved for that whole time.
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

/// RFC 6455 §7.4.1 **1001 Going Away**, the code it gives for "a server going
/// down". The client reconnects, to a replica still in the load balancer, and
/// subscribes again.
const GOING_AWAY: &str = "the server is going away, reconnect to continue";

/// §7.4.1 **1001 Going Away** too: the server is deliberately ending a socket it
/// will no longer serve, and what it asks for is a fresh upgrade — which re-runs
/// the guard and re-checks `exp`. The sentence a WebSocket gateway closes with.
const LIFETIME_REACHED: &str = "connection lifetime reached, re-upgrade to continue";

/// §7.4.1 **1011 Internal Error** — a subscription unwound, an unexpected
/// condition that prevented the server from serving the rest of the socket.
const UNWOUND: &str = "the subscription failed on the server";

/// The end the socket's server half arms: the shutdown signal, or the lifetime
/// ceiling, whichever comes first — `None` ⇒ no ceiling.
///
/// The ceiling is a security control: a principal captured once at the upgrade
/// would otherwise keep its privileges after token expiry, logout or
/// revocation, for as long as the peer holds the socket open. The deadline is
/// **absolute**, armed once: a ceiling on the connection's life, not an idle
/// timeout, so traffic never pushes it out.
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
            // `debug`: the deployment asked for this and said so once; a line
            // per socket at `info` is the fleet's size in noise. The socket's
            // `graphql.subscription` line still files, `cancelled`.
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

/// The graphql-ws endpoint served on the same path as the POST endpoint.
///
/// It is reached through [`crate::module`]'s GET dispatcher, which sends a
/// WebSocket upgrade here and everything else to the playground — one URL, the
/// shape every graphql-ws client expects.
pub(crate) struct SubscriptionEndpoint<E> {
    executor: E,
    bridge: Arc<OperationBridge>,
    max_connection: Option<Duration>,
    /// Every socket this mount serves. poem stops tracking a connection at its
    /// upgrade, so without this the shutdown window neither waited for a socket
    /// nor closed it, and every subscription ran on under the shutdown hooks.
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
        // The guard runs on the **upgrade**, before any operation exists: this
        // is where the principal is established, exactly as `before` does on the
        // POST path. A denial is answered as an ordinary HTTP response, so the
        // socket is never opened.
        //
        // `around` runs too, below — over the whole socket rather than over one
        // operation.
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
        // The guard's ambient state is installed around the **whole socket**,
        // not per operation: one upgrade, one principal, for every operation and
        // every item the connection carries. That is the same model a WebSocket
        // gateway uses, and it is what makes `Guard::check_graphql` and the
        // ability-scoped data layer read the same ability here as on the POST
        // path — `around` never runs otherwise, since a socket has no response
        // future to wrap.
        let guard = self.bridge.op_guard.clone();
        // Read here, on the request task, for the same reason the executor is.
        let correlation = socket_correlation();
        let sockets = self.sockets.clone();

        Ok(websocket
            .protocols(ALL_WEBSOCKET_PROTOCOLS)
            .on_upgrade(move |stream| async move {
                // One span for the whole socket, and that granularity is the
                // honest one: async-graphql's protocol engine dispatches the
                // operations, so this crate never sees an operation boundary to
                // open a span at. Where it *does* see one — a WebSocket gateway's
                // messages — the message is the unit and the connection is a
                // field. Here the connection is all there is.
                let span = nest_rs_core::operation_span!(crate::unit::SUBSCRIPTION, &correlation,);
                let line = SubscriptionLine::new(correlation.clone(), span.clone());
                let end = ends_at(sockets.going_away(), max_connection);
                // The socket answers through a local slot because the guard
                // scopes a `()` future — see `GraphqlOperationGuard::around` —
                // and the end is the line's to report.
                let mut ended: Option<Ended> = None;
                let serving: crate::BoxFuture<'_, ()> = Box::pin(async {
                    let served = serve_socket(stream, executor, protocol.0, data, end);
                    ended = Some(match max_connection {
                        // The ceiling's hard edge: a socket that has not taken its
                        // close within the grace is dropped, as the ceiling always
                        // did — a peer that stopped reading cannot hold it open.
                        // At the signal the window is that edge.
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
                // Executor outermost, ability inside — the same nesting the POST
                // path uses (`without_transaction` over the guarded operation),
                // so the two cannot come to disagree about which is in scope.
                let served: crate::BoxFuture<'_, ()> = match socket_executor {
                    Some(executor) => {
                        Box::pin(nest_rs_database::with_request_executor(executor, guarded))
                    }
                    None => guarded,
                };
                // Carried by the transport: told at the signal, given the window
                // to finish, and dropped where it waits if it has not by the
                // window's close — which `line`'s `Drop` files `cancelled`.
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
/// it, or `end` resolves — and end it the protocol's way, which async-graphql's
/// own loop has no seam for: every subscription still running is completed,
/// what is still answering — a query or a mutation sent over the socket — is
/// answered, then the socket closes with 1001.
///
/// The completions are async-graphql's own. At `end`, the inbound half
/// ([`Inbound`]) stops reading the peer and hands the protocol engine a `stop`
/// for every subscription the peer started and nobody finished; the engine
/// answers each still-running one with `complete` — in the negotiated
/// protocol's wording, `graphql-transport-ws` and the legacy `graphql-ws` alike
/// — and ignores the rest. So no `complete` is written twice, and none is
/// written in a shape this crate spelled. The engine ends once nothing is left
/// answering, and the socket with it.
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
        // A subscription's stream is developer code polled right here, so this
        // is where one that panics is contained: the socket closes with 1011,
        // where an unwinding task left it 1006 and filed no line.
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
            // The peer ended it. Its Close is echoed by the protocol layer, and
            // the flush is what puts that echo on the wire.
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

/// What an operation sent over the socket is, for how the socket's end treats
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    /// A subscription has no end of its own, so the socket's end completes it.
    Subscription,
    /// A query or a mutation is still answering, so it is answered first —
    /// inside the window at shutdown, as a request still running is.
    Answer,
}

impl Operation {
    /// Read off the request's document — which async-graphql caches on the
    /// request and reuses when it executes it, so this is not a second parse.
    /// A document that does not parse, or names no operation it holds, is
    /// answered with an error at once, so it is an [`Operation::Answer`].
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
/// Read off the frame's opening rather than by parsing every frame: a `next`
/// carries a result of any size, and is by far the commonest frame. serde writes
/// an internally tagged enum's tag first, and async-graphql's `complete` is that
/// enum with nothing but an `id` beside it, so the opening is exact — and pinned
/// by a test against async-graphql's own output. Missing one only leaves an id
/// in [`Started`]: the engine ignores a `stop` for an operation it has finished.
fn completed_id(frame: &str) -> Option<String> {
    if !frame.starts_with(r#"{"type":"complete""#) {
        return None;
    }
    // A `complete` is a type and an id, so reading it whole costs nothing.
    match serde_json::from_str::<serde_json::Value>(frame)
        .ok()?
        .get("id")?
    {
        serde_json::Value::String(id) => Some(id.clone()),
        _ => None,
    }
}

/// The peer's half of a socket, as the protocol engine reads it — and the one
/// place the server's end of the socket can enter the protocol.
///
/// It parses each frame as the engine would, so it can track which operations
/// are running; once `end` resolves it reads the peer no more, yields a `stop`
/// for each running subscription instead, and ends once nothing is left
/// answering.
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
            // Every subscription is stopped. What is still answering keeps
            // going; the engine wakes on its output, and asks again — each
            // `complete` it writes is pruned before it does.
            return if lock(&this.started).is_empty() {
                Poll::Ready(None)
            } else {
                Poll::Pending
            };
        }
        loop {
            match std::pin::Pin::new(&mut this.socket).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                // A read error ends the peer's half, as it ends the engine's
                // own reading of it.
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

/// Files the subscription's line when the connection ends.
///
/// **On drop, not after the await**, and that distinction is the whole reason
/// this type exists rather than a `tracing::info!` at the end of the block: a
/// socket that is aborted — the transport stopping it at the close of its
/// window, the runtime going down, its task unwinding — never completes, so its
/// future is dropped rather than finished, and that end is `cancelled` (or
/// `panic`). A socket that does finish says how through [`settle`](Self::settle).
///
/// The correlation is held rather than read from the ambient context: a `Drop`
/// runs while the future is being torn down, which is not reliably inside the
/// scope that future installed.
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

    /// Say how the socket ended. `ok` when its peer ended it, `error` when the
    /// protocol refused it, `cancelled` when the server ended it — the
    /// subscriber did not — and `panic` when a subscription unwound, which is
    /// also told to the operator here.
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
            // Its duration is how long it stayed open, which on a subscription
            // is the number an operator actually reads.
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

    /// The line is filed from `Drop`, so two properties matter more than what it
    /// says: it fires **once**, and it cannot panic.
    ///
    /// A panic inside a `Drop` that runs during an unwind aborts the process, and
    /// this `Drop` reaches a `tokio` task-local. A socket is torn down on paths
    /// this crate does not choose — the runtime shutting down, the transport
    /// stopping it, a handler unwinding — so "does it work on the happy path" is
    /// not the question. A socket dropped before it said how it ended was
    /// stopped, and says `cancelled`.
    #[test]
    fn the_subscription_line_fires_once_and_cannot_panic_off_a_runtime() {
        let logs = nest_rs_testing::LogCapture::install();

        // No `#[tokio::test]`: this is the teardown case, where there may be no
        // runtime left to reach a task-local through.
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

    /// Each end the socket loop sees is its own word, and the line files it
    /// once.
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

    /// And once inside a runtime too, since that is where it normally runs — the
    /// two paths reach the task-local differently and only one of them is the
    /// happy one.
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

    /// A request's lazy transaction handle: `non_transactional` steps out of it
    /// onto the pool, exactly as the ORM's does.
    struct Transaction;
    /// The pool handle it steps out onto — already outside a transaction, so it
    /// has nothing to step out of.
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

    /// The rule a socket cannot break: a connection that may live for hours
    /// never carries the upgrade's transaction. Carrying it would pin a pooled
    /// connection for that whole time and write through a transaction the `101`
    /// already finalized.
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

    /// The identity half of the same capture. Every event a subscription
    /// produces — an item withheld, a mask that failed — is filed in the trace
    /// that opened the socket, so "what did this subscriber see?" is one query
    /// against the trace the client already has from its upgrade.
    #[tokio::test]
    async fn a_socket_runs_in_the_upgrades_trace() {
        let upgrade = Correlation::minted(None);
        let captured =
            nest_rs_core::with_request_scope(None, upgrade.clone(), async { socket_correlation() })
                .await;

        assert_eq!(captured.trace_id(), upgrade.trace_id());
    }

    /// And its actor, because nothing on a socket re-authenticates: what the
    /// upgrade's guard resolved is the only answer there will ever be.
    #[tokio::test]
    async fn a_socket_runs_under_the_upgrades_actor() {
        let captured = nest_rs_core::with_request_scope(None, Correlation::minted(None), async {
            nest_rs_core::set_actor_id("alice-42");
            socket_correlation()
        })
        .await;

        assert_eq!(captured.actor_id(), Some("alice-42"));
    }

    /// Mounted outside the HTTP edge there is nothing to inherit. A fresh id
    /// still groups the socket's own events with each other, which is the point
    /// — an unidentified socket is the one outcome that helps nobody.
    #[tokio::test]
    async fn a_socket_with_no_upgrade_to_inherit_from_starts_its_own_trace() {
        assert!(nest_rs_core::current_trace_id().is_none());
        assert!(socket_correlation().actor_id().is_none());
    }

    /// The other half: a handle that is *already* the pool is kept, rather than
    /// dropped for want of something to step out of — which would leave a
    /// `Repo`-backed subscription with no executor at all.
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

    /// No ORM installed at all: nothing to carry, and the socket runs untouched
    /// rather than failing to open.
    #[tokio::test]
    async fn a_socket_without_an_orm_carries_nothing() {
        assert!(socket_executor().is_none());
    }

    /// [`completed_id`] reads a frame's opening rather than parsing every frame,
    /// so the opening it reads is pinned here against the frame async-graphql
    /// itself writes for an operation that completed — in both protocols the
    /// mount negotiates.
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

    /// At its end the peer's half stops reading and hands the engine a `stop`
    /// for every operation still running, then ends — which is how the engine
    /// comes to write a `complete` for each.
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

    /// A query or a mutation sent over the socket is still answering at the
    /// socket's end, so it is not stopped: the peer's half waits for its
    /// `complete` — pruned by the outbound half — before it ends.
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
