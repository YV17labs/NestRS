//! The graphql-ws half of the `/graphql` mount, and the per-item posture seam
//! `#[subscription]` expands into.
//!
//! A subscription goes through the same gate as every operation, decided once at
//! subscribe, so every item is filtered against the ability captured then
//! ([`keep_masked_item`]). The socket is bounded as a gateway's is: a browser
//! origin checked at the upgrade, a lifetime ceiling, a message cap, a wait
//! for `connection_init` and a count of operations running at once
//! ([`GraphqlConfig`]).

use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use async_graphql::http::{ALL_WEBSOCKET_PROTOCOLS, ClientMessage, WebSocketProtocols, WsMessage};
use async_graphql::parser::types::OperationType;
use async_graphql::{Data, Executor};
use futures_util::future::Either;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, Stream, StreamExt};

use nest_rs_core::{Container, Continuation, Correlation, RequestContinuation};
use nest_rs_http::__private::websocket::{
    self, CloseCode, HandshakeRefused, Message, WebSocketLimits, WebSocketStream, WebSocketUpgrade,
};
use nest_rs_http::{DetachedWork, Endpoint, Request, Response};
use tracing::Instrument;

use crate::config::GraphqlConfig;
use crate::context::{ENDPOINT, OperationBridge, fold_variables, variable_pipe_error};
use nest_rs_http::__private::with_view;
use nest_rs_pipes::{GlobalPipe, PipeError};

/// The composed schema as a bare [`Executor`], as the mount serves it — each
/// operation run as its own unit: a graphql-ws driver
/// ([`async_graphql::http::WebSocket`]) takes an executor, not a URL, so a
/// test subscriber needs this.
pub fn compose_schema(container: Container, config: &GraphqlConfig) -> impl Executor + 'static {
    let binding = container.get_dyn::<dyn nest_rs_database::UnitOfWork>();
    let schema = crate::resolver::build_schema(container, config, &DetachedWork::new());
    let sdl = schema.sdl();
    crate::operation_units::OperationUnits::new(
        crate::null_propagation::NullPropagated::new(crate::redact::Redacted(schema), &sdl),
        binding,
    )
}

/// Keep or drop one masked subscription item, called per item by the
/// `#[subscription]` expansion: a refused row is dropped at `debug`; a mask
/// failure is dropped at `warn`, since an item has no error channel.
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

/// The data handle a socket runs on when no unit-of-work binding is mounted,
/// taken from the **upgrade request**: the `101` is answered before the
/// connection task runs.
///
/// Always **non-transactional**: the request transaction is finalized with the
/// `101`, and carrying it onto an hours-long socket would pin a pooled connection
/// behind a transaction nobody commits.
fn socket_executor() -> Option<Arc<dyn nest_rs_database::Executor>> {
    nest_rs_database::current_executor()
        .map(|executor| executor.non_transactional().unwrap_or(executor))
}

/// How a socket reaches the data layer, read off the **upgrade request**.
enum SocketData {
    /// The app's unit-of-work binding: the socket runs as a
    /// [`Pool`](nest_rs_database::TransactionPolicy::Pool) unit of it, so a
    /// DataLoader batch its operations continue into reaches that binding's
    /// pool too.
    Bound(Arc<dyn nest_rs_database::UnitOfWork>),
    /// An executor the upgrade ran on outside any unit, stepped out of its
    /// transaction ([`socket_executor`]).
    Installed(Arc<dyn nest_rs_database::Executor>),
}

impl SocketData {
    /// The mount's `binding`, else what the upgrade request ran on; `None`
    /// without a data layer.
    fn of_upgrade(binding: Option<&Arc<dyn nest_rs_database::UnitOfWork>>) -> Option<Self> {
        match binding {
            Some(binding) => Some(Self::Bound(Arc::clone(binding))),
            None => socket_executor().map(Self::Installed),
        }
    }

    /// `socket`, run on this data layer.
    fn serve<'a>(self, socket: crate::BoxFuture<'a, ()>) -> crate::BoxFuture<'a, ()> {
        match self {
            Self::Bound(binding) => Box::pin(async move {
                let unit = nest_rs_database::UnitSpec::new(
                    crate::unit::SUBSCRIPTION,
                    nest_rs_database::ExecutorScope::Request,
                    nest_rs_database::TransactionPolicy::Pool,
                );
                nest_rs_database::run_unit(Some(&binding), unit, socket, |_| true, refused).await;
            }),
            Self::Installed(executor) => {
                Box::pin(nest_rs_database::with_request_executor(executor, socket))
            }
        }
    }
}

/// A binding that refused the socket its pool: the socket closes unserved.
fn refused(why: nest_rs_database::Unhonoured) {
    tracing::error!(
        target: crate::TARGET,
        error = %nest_rs_core::error_message(&why),
        "subscription socket not served: the unit-of-work binding refused its pool",
    );
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

/// What an operation past the socket's cap is answered with.
const TOO_MANY_OPERATIONS: &str = "too many operations on this socket";

/// Why the server refused a socket mid-protocol, each with the close the peer
/// is told it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// No `connection_init` within the wait — graphql-ws `PROTOCOL.md`,
    /// *ConnectionInit*.
    InitTimeout,
    /// A `subscribe` reusing the id of a running operation — *Subscribe*. The
    /// reason does not echo the id.
    SubscriberExists,
    /// The read failed — a frame past the message cap (1009), or broken framing
    /// (1011) — with the close the gateway's socket ends with too.
    ReadFailed(CloseCode, &'static str),
}

impl Refusal {
    fn close(self) -> (CloseCode, &'static str) {
        match self {
            Self::InitTimeout => (CloseCode::from(4408), "Connection initialisation timeout"),
            Self::SubscriberExists => (CloseCode::from(4409), "Subscriber already exists"),
            Self::ReadFailed(code, reason) => (code, reason),
        }
    }

    /// How the socket's line files it: a read that failed is most often the
    /// connection gone, which a peer ends.
    fn ended(self) -> Ended {
        match self {
            Self::InitTimeout | Self::SubscriberExists => Ended::Refused,
            Self::ReadFailed(..) => Ended::ByPeer,
        }
    }
}

/// What one socket is held to, read once off [`GraphqlConfig`].
#[derive(Clone, Copy, Debug)]
struct SocketBounds {
    /// The lifetime ceiling; `None` ⇒ unlimited.
    max_connection: Option<Duration>,
    /// The per-message and per-frame byte cap, at the protocol layer.
    max_message_bytes: usize,
    /// The wait for `connection_init`.
    connection_init_timeout: Duration,
    /// Operations running at once; `None` ⇒ unlimited.
    max_operations: Option<usize>,
}

impl SocketBounds {
    fn of(config: &GraphqlConfig) -> Self {
        Self {
            max_connection: config.max_connection,
            max_message_bytes: config.max_message_bytes,
            connection_init_timeout: config.connection_init_timeout,
            max_operations: config.max_socket_operations.filter(|&max| max > 0),
        }
    }
}

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
    /// It broke the protocol — async-graphql refused an unacknowledged
    /// connection, a malformed message or a repeated `connection_init`; the
    /// server, a missing `connection_init` or a reused id.
    Refused,
    /// The server ended it: its subscriptions completed, then 1001 — or it
    /// dropped the socket, whose peer took nothing within the send deadline or
    /// did not take its close within the grace.
    ByServer,
    /// A subscription unwound; the socket closed with 1011.
    Unwound(Box<dyn std::any::Any + Send>),
}

/// The graphql-ws endpoint, reached through the GET dispatcher on the POST
/// endpoint's path: the handshake on the transport's upgrade, the subprotocol
/// the first of the client's offers async-graphql speaks.
pub(crate) struct SubscriptionEndpoint<E> {
    executor: E,
    bridge: Arc<OperationBridge>,
    bounds: SocketBounds,
    /// Every socket this mount serves: an upgraded socket outlives the HTTP
    /// connection the shutdown window waits on, so the window waits on this too.
    sockets: DetachedWork,
    /// The unit-of-work binding, resolved once at mount.
    unit_of_work: Option<Arc<dyn nest_rs_database::UnitOfWork>>,
}

impl<E> SubscriptionEndpoint<E> {
    pub(crate) fn new(
        executor: E,
        bridge: Arc<OperationBridge>,
        config: &GraphqlConfig,
        sockets: DetachedWork,
        unit_of_work: Option<Arc<dyn nest_rs_database::UnitOfWork>>,
    ) -> Self {
        Self {
            executor,
            bridge,
            bounds: SocketBounds::of(config),
            sockets,
            unit_of_work,
        }
    }
}

impl<E: Executor> Endpoint for SubscriptionEndpoint<E> {
    async fn call(&self, mut req: Request) -> nest_rs_http::Result<Response> {
        // The transport checked the origin and ran the upgrade's guards in front:
        // a refusal there is an ordinary HTTP response, and no socket opens.
        let upgrade = WebSocketUpgrade::from_request(&mut req)?;
        let protocol = upgrade
            .protocols()
            .find_map(|offered| WebSocketProtocols::from_str(offered).ok())
            .ok_or(HandshakeRefused::SubprotocolNotOffered(
                &ALL_WEBSOCKET_PROTOCOLS,
            ))?;
        let pipes = SocketPipes::of(&self.bridge, &req);
        // What the upgrade's guards attached reaches the socket the way it
        // reaches a POST: forwarded by the seeds, carried by the connection.
        // Never the upgrade's scope nor its transaction: the socket's own
        // executor is installed below.
        let data = self.bridge.connection_data(&req);
        let carried = RequestContinuation::capture(Continuation::Connection);
        let executor = self.executor.clone();
        let bounds = self.bounds;
        let socket_data = SocketData::of_upgrade(self.unit_of_work.as_ref());
        // Read on the request task, before the `101` is answered.
        let correlation = socket_correlation();
        let sockets = self.sockets.clone();
        // At the protocol layer, so a frame past the cap is never buffered whole.
        let limits = WebSocketLimits::new(bounds.max_message_bytes);

        Ok(upgrade.accept(
            Some(protocol.sec_websocket_protocol()),
            limits,
            move |stream| async move {
                // One span per socket: async-graphql's engine hides operation
                // boundaries from this crate.
                let span = nest_rs_core::operation_span!(crate::unit::SUBSCRIPTION, &correlation,);
                let line = SubscriptionLine::new(correlation.clone(), span.clone());
                let end = ends_at(sockets.going_away(), bounds.max_connection);
                // A local slot, since the executor scope wraps a `()` future.
                let mut ended: Option<Ended> = None;
                let serving: crate::BoxFuture<'_, ()> = Box::pin(async {
                    let served = serve_socket(stream, executor, protocol, data, end, bounds, pipes);
                    ended = Some(match bounds.max_connection {
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
                let served = match socket_data {
                    Some(socket_data) => socket_data.serve(serving),
                    None => serving,
                };
                // Dropped at the window's close, which `line`'s `Drop` files `cancelled`.
                let served = match carried {
                    Some(carried) => Either::Left(carried.scope(served)),
                    None => {
                        Either::Right(nest_rs_core::with_request_scope(None, correlation, served))
                    }
                };
                let _ = sockets.run(served).instrument(span).await;
                if let Some(ended) = ended {
                    line.settle(ended);
                }
            },
        ))
    }
}

/// Serve one graphql-ws socket until its peer ends it, the protocol refuses
/// it, or `end` resolves — then complete every running subscription, answer
/// what is still answering, and close with 1001, a seam async-graphql's loop
/// lacks.
///
/// At `end`, [`Inbound`] hands the engine a `stop` per running subscription,
/// so the engine writes each `complete` in the negotiated protocol's wording.
/// A `subscribe` past the operation cap, or one whose variables a global pipe
/// refuses, never reaches the engine: this loop answers it the protocol's
/// `error` for its id.
async fn serve_socket<E: Executor>(
    stream: WebSocketStream,
    executor: E,
    protocol: WebSocketProtocols,
    data: Data,
    end: impl Future<Output = Ending> + Send + 'static,
    bounds: SocketBounds,
    pipes: Option<SocketPipes>,
) -> Ended {
    let (mut sink, socket) = stream.split();
    let inbound = Inbound::new(socket, end, bounds, pipes);
    let started = Arc::clone(&inbound.started);
    let ending = Arc::clone(&inbound.ending);
    let refusal = Arc::clone(&inbound.refusal);
    let withheld = Arc::clone(&inbound.withheld);
    let mut engine =
        async_graphql::http::WebSocket::from_message_stream(executor, inbound, protocol)
            .connection_data(data);
    loop {
        if let Some(frame) = withheld.take() {
            if let Err(error) = sink.send(Message::text(frame)).await {
                return stopped_writing(&error);
            }
            continue;
        }
        // A subscription's stream is developer code polled here: a panic closes
        // the socket with 1011.
        let next = tokio::select! {
            () = withheld.ready.notified() => continue,
            next = nest_rs_core::panic::contain(engine.next()) => next,
        };
        let next = match next {
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
                if let Err(error) = sink.send(Message::text(text)).await {
                    return stopped_writing(&error);
                }
            }
            Some(WsMessage::Close(code, reason)) => {
                close_socket(&mut sink, CloseCode::from(code), &reason).await;
                return Ended::Refused;
            }
            None => break,
        }
    }
    if let Some(refusal) = refusal.get() {
        let (code, reason) = refusal.close();
        close_socket(&mut sink, code, reason).await;
        return refusal.ended();
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

/// A frame could not be written: past the transport's send deadline the
/// server drops the socket; any other failure is the peer gone.
fn stopped_writing(error: &websocket::Error) -> Ended {
    if !matches!(error, websocket::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut) {
        return Ended::ByPeer;
    }
    tracing::warn!(
        target: crate::TARGET,
        error = %nest_rs_core::error_message(error),
        "the peer took nothing within the send deadline; it is cut off",
    );
    Ended::ByServer
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
    /// Read off the operation the request selects, as the engine selects it.
    /// One that selects none is answered at once, so it is an [`Operation::Answer`].
    fn of(request: &mut async_graphql::Request) -> Self {
        match crate::operation_units::selected_type(request) {
            Some(OperationType::Subscription) => Self::Subscription,
            _ => Self::Answer,
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

/// The `subscribe` refused before it ran — past the operation cap, or by a
/// global pipe — that waits for the socket's loop to answer it. The peer's
/// half reads nothing more while one waits, so one at most ever does.
#[derive(Default)]
struct Withheld {
    /// The protocol's `error` message answering it.
    frame: Mutex<Option<String>>,
    /// Wakes the loop, which the engine never would: it saw no message.
    ready: tokio::sync::Notify,
}

impl Withheld {
    fn hold(&self, frame: String) {
        *self.slot() = Some(frame);
        self.ready.notify_one();
    }

    fn take(&self) -> Option<String> {
        self.slot().take()
    }

    fn waits(&self) -> bool {
        self.slot().is_some()
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The protocol's `error` message for an operation refused before it ran,
/// which ends that operation alone; the id is the protocol's correlation key.
fn operation_refused(id: &str, errors: serde_json::Value) -> String {
    serde_json::json!({
        "type": "error",
        "id": id,
        "payload": errors,
    })
    .to_string()
}

/// The global pipes a socket's operations meet, and the view they read them
/// through: the head of the request that opened the socket, which serves the
/// socket's guards for its whole lifetime and so cannot be lent per operation.
pub(crate) struct SocketPipes {
    pipes: Arc<[Arc<dyn GlobalPipe>]>,
    opened: nest_rs_http::Request,
}

impl SocketPipes {
    /// The pool `bridge` resolved at mount, over the head of `req` — `None`
    /// when the pool is empty, so a socket with no pipe copies nothing.
    fn of(bridge: &OperationBridge, req: &nest_rs_http::Request) -> Option<Self> {
        let pipes = bridge.pipes();
        if pipes.is_empty() {
            return None;
        }
        let mut opened = nest_rs_http::Request::builder()
            .method(req.method().clone())
            .uri(req.uri().clone());
        if let Some(peer) = req.peer_addr() {
            opened = opened.peer_addr(peer);
        }
        let mut opened = opened.finish();
        *opened.headers_mut() = req.headers().clone();
        Some(Self {
            pipes: Arc::clone(pipes),
            opened,
        })
    }

    /// Hand the pipes `operation`'s variables, as the `/graphql` mount's unit,
    /// and put back what they made of them.
    fn fold(
        &mut self,
        operation: &mut async_graphql::Request,
    ) -> std::result::Result<(), PipeError> {
        let pipes = &self.pipes;
        operation.variables = with_view(&ENDPOINT, &mut self.opened, |cx| {
            fold_variables(cx, &operation.variables, pipes)
        })?;
        Ok(())
    }
}

/// The peer's half of a socket, as the protocol engine reads it, tracking which
/// operations run; once `end` resolves it yields a `stop` per running
/// subscription instead, and ends once nothing is left answering.
///
/// It also holds the socket to its bounds: it ends the socket with a
/// [`Refusal`] past the wait for `connection_init`, on a reused id or on a
/// failed read, and withholds a `subscribe` past the operation cap.
struct Inbound<S> {
    socket: S,
    end: std::pin::Pin<Box<dyn Future<Output = Ending> + Send>>,
    started: Started,
    ending: Arc<OnceLock<Ending>>,
    /// The `stop`s left to hand the engine, once `end` has resolved.
    stopping: Option<std::vec::IntoIter<String>>,
    /// Armed at the upgrade, cleared by the first `connection_init`.
    init_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    max_operations: Option<usize>,
    withheld: Arc<Withheld>,
    refusal: Arc<OnceLock<Refusal>>,
    /// The global pipes each operation's variables go through, if any.
    pipes: Option<SocketPipes>,
}

impl<S> Inbound<S> {
    fn new(
        socket: S,
        end: impl Future<Output = Ending> + Send + 'static,
        bounds: SocketBounds,
        pipes: Option<SocketPipes>,
    ) -> Self {
        Self {
            socket,
            end: Box::pin(end),
            started: Started::default(),
            ending: Arc::default(),
            stopping: None,
            init_deadline: Some(Box::pin(tokio::time::sleep(bounds.connection_init_timeout))),
            max_operations: bounds.max_operations,
            withheld: Arc::default(),
            refusal: Arc::default(),
            pipes,
        }
    }

    /// End the peer's half, so the engine ends and the loop closes with
    /// `refusal`.
    fn refuse<T>(&self, refusal: Refusal) -> Poll<Option<T>> {
        #[expect(clippy::let_underscore_must_use, reason = "the first refusal stands")]
        let _ = self.refusal.set(refusal);
        Poll::Ready(None)
    }
}

impl<S> Stream for Inbound<S>
where
    S: Stream<Item = Result<Message, websocket::Error>> + Unpin,
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
        if let Some(deadline) = this.init_deadline.as_mut()
            && deadline.as_mut().poll(cx).is_ready()
        {
            tracing::debug!(
                target: crate::TARGET,
                close_code = 4408,
                "closing subscription socket: no connection_init within the wait",
            );
            return this.refuse(Refusal::InitTimeout);
        }
        // The loop answers the withheld `subscribe`, then polls the engine again.
        if this.withheld.waits() {
            return Poll::Pending;
        }
        loop {
            match std::pin::Pin::new(&mut this.socket).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(error))) => {
                    let (code, reason) = websocket::read_failed(&error);
                    tracing::debug!(
                        target: crate::TARGET,
                        error = %nest_rs_core::error_message(&error),
                        close_code = u16::from(code),
                        "subscription socket read failed",
                    );
                    return this.refuse(Refusal::ReadFailed(code, reason));
                }
                Poll::Ready(Some(Ok(message))) => {
                    if !(message.is_text() || message.is_binary()) {
                        continue;
                    }
                    let mut parsed = serde_json::from_slice::<ClientMessage>(&message.into_data())
                        .map_err(not_a_message);
                    match &mut parsed {
                        Ok(ClientMessage::ConnectionInit { .. }) => this.init_deadline = None,
                        Ok(ClientMessage::Start { id, payload }) => {
                            let mut started = lock(&this.started);
                            if started.contains_key(id.as_str()) {
                                drop(started);
                                tracing::debug!(
                                    target: crate::TARGET,
                                    close_code = 4409,
                                    "closing subscription socket: a running operation's id \
                                     was reused",
                                );
                                return this.refuse(Refusal::SubscriberExists);
                            }
                            if let Some(max) = this.max_operations
                                && started.len() >= max
                            {
                                drop(started);
                                tracing::debug!(
                                    target: crate::TARGET,
                                    max_socket_operations = max,
                                    "subscribe refused: the socket runs its maximum of operations",
                                );
                                this.withheld.hold(operation_refused(
                                    id,
                                    serde_json::json!([{ "message": TOO_MANY_OPERATIONS }]),
                                ));
                                return Poll::Pending;
                            }
                            if let Some(pipes) = this.pipes.as_mut() {
                                // Developer code runs outside the lock.
                                drop(started);
                                if let Err(refused) = pipes.fold(payload) {
                                    tracing::debug!(
                                        target: crate::TARGET,
                                        reason = "global_pipe",
                                        "subscribe refused: a global pipe refused its variables",
                                    );
                                    this.withheld.hold(operation_refused(
                                        id,
                                        serde_json::json!([variable_pipe_error(&refused)]),
                                    ));
                                    return Poll::Pending;
                                }
                                started = lock(&this.started);
                            }
                            started.insert(id.clone(), Operation::of(payload));
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

/// What a frame that is not a graphql-ws message is refused as.
const NOT_A_MESSAGE: &str = "not a graphql-ws message";

/// A frame that does not decode, as the engine is handed it: the engine closes
/// the socket with the error's sentence as the reason, and serde's sentence
/// quotes the payload it refused.
fn not_a_message(error: serde_json::Error) -> serde_json::Error {
    tracing::debug!(
        target: crate::TARGET,
        error = %nest_rs_core::DecodeError::new(&error),
        "graphql-ws frame refused: it does not decode as a protocol message",
    );
    <serde_json::Error as nest_rs_core::__private::serde::de::Error>::custom(NOT_A_MESSAGE)
}

/// Put a Close frame on the wire, then flush — best-effort, since a peer that
/// has already gone cannot be told anything. An engine sentence past what a
/// control frame holds is cut, never sent whole for the peer to refuse.
async fn close_socket(
    sink: &mut SplitSink<WebSocketStream, Message>,
    code: CloseCode,
    reason: &str,
) {
    if let Err(err) = sink.send(websocket::close_frame(code, reason)).await {
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

    #[test]
    fn a_frame_that_is_not_a_protocol_message_is_refused_without_its_payload() {
        let refused = serde_json::from_slice::<ClientMessage>(
            br#"{"type":"subscribe","id":"1","payload":"sk_live_secret"}"#,
        )
        .map_err(not_a_message)
        .err()
        .expect("a string is no request payload");
        assert_eq!(refused.to_string(), NOT_A_MESSAGE);
    }

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
            nest_rs_core::__private::set_actor_id("alice-42");
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
        assert!(SocketData::of_upgrade(None).is_none());
    }

    /// A binding opening the request's transaction, or the pool.
    struct Binding;
    /// A unit [`Binding`] opened.
    struct Opened(nest_rs_database::TransactionPolicy);

    impl nest_rs_database::UnitOfWork for Binding {
        fn open(
            &self,
            spec: &nest_rs_database::UnitSpec,
        ) -> Result<Box<dyn nest_rs_database::OpenUnit>, nest_rs_database::Unhonoured> {
            Ok(Box::new(Opened(spec.policy)))
        }

        fn settle_net(&self) -> std::time::Duration {
            std::time::Duration::from_secs(16)
        }
    }

    impl nest_rs_database::OpenUnit for Opened {
        fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
            match self.0 {
                nest_rs_database::TransactionPolicy::Pool => Arc::new(Pool),
                _ => Arc::new(Transaction),
            }
        }

        fn settle(
            self: Box<Self>,
            _ok: bool,
        ) -> std::pin::Pin<Box<dyn Future<Output = nest_rs_database::Settlement> + Send>> {
            Box::pin(std::future::ready(nest_rs_database::Settlement::Settled))
        }
    }

    #[tokio::test]
    async fn a_batch_under_a_socket_runs_on_the_pool_of_the_upgrades_binding() {
        let binding: Arc<dyn nest_rs_database::UnitOfWork> = Arc::new(Binding);
        let upgrade = nest_rs_database::UnitSpec::new(
            crate::unit::OPERATION,
            nest_rs_database::ExecutorScope::Request,
            nest_rs_database::TransactionPolicy::PerUnit,
        );
        let socket_data = nest_rs_database::run_unit(
            Some(&binding),
            upgrade,
            async { SocketData::of_upgrade(Some(&binding)) },
            |_| true,
            |_| None,
        )
        .await
        .expect("the upgrade runs inside its unit");

        let mut socket_ran_on_the_pool = false;
        let mut batch_ran_on = None;
        let socket = socket_data.serve(Box::pin(async {
            socket_ran_on_the_pool = nest_rs_database::current_executor()
                .is_some_and(|executor| executor.as_any().is::<Pool>());
            let batch = RequestContinuation::capture(Continuation::Batch)
                .expect("a socket runs in a request scope");
            batch_ran_on = Some(
                batch
                    .scope(async {
                        nest_rs_database::current_executor()
                            .is_some_and(|executor| executor.as_any().is::<Pool>())
                    })
                    .await,
            );
        }));
        nest_rs_core::with_request_scope(None, Correlation::minted(None), socket).await;

        assert!(
            socket_ran_on_the_pool,
            "the socket never carries the upgrade's transaction"
        );
        assert_eq!(
            batch_ran_on,
            Some(true),
            "a DataLoader batch under the socket runs on the binding's pool",
        );
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

    fn bounds() -> SocketBounds {
        SocketBounds::of(&GraphqlConfig::default())
    }

    /// The engine runs a document's one operation without its name, so the
    /// socket's end must complete it like any other subscription.
    #[test]
    fn a_lone_named_subscription_sent_without_its_name_is_a_subscription() {
        let mut named = async_graphql::Request::new("subscription Ticks { x }");
        assert_eq!(Operation::of(&mut named), Operation::Subscription);
    }

    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test waits for the end; how it ended is asserted elsewhere"
    )]
    async fn at_its_end_the_peers_half_stops_every_running_operation_and_ends() {
        use async_graphql::futures_util::stream;

        let frames = stream::iter([
            Ok::<_, websocket::Error>(Message::text(r#"{"type":"connection_init"}"#)),
            Ok(Message::text(
                r#"{"id":"a","type":"subscribe","payload":{"query":"subscription { x }"}}"#,
            )),
            Ok(Message::text(
                r#"{"id":"b","type":"subscribe","payload":{"query":"subscription { x }"}}"#,
            )),
            Ok(Message::text(r#"{"id":"a","type":"complete"}"#)),
        ])
        .chain(stream::pending());
        let (end_now, ended) = tokio::sync::oneshot::channel::<()>();
        let mut inbound = Inbound::new(
            Box::pin(frames),
            async move {
                let _ = ended.await;
                Ending::GoingAway
            },
            bounds(),
            None,
        );
        let ending = Arc::clone(&inbound.ending);
        for _ in 0..4 {
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
            Ok::<_, websocket::Error>(Message::text(r#"{"type":"connection_init"}"#)),
            Ok(Message::text(
                r#"{"id":"s","type":"subscribe","payload":{"query":"subscription { x }"}}"#,
            )),
            Ok(Message::text(
                r#"{"id":"q","type":"subscribe","payload":{"query":"mutation { y }"}}"#,
            )),
        ])
        .chain(stream::pending());
        let (end_now, ended) = tokio::sync::oneshot::channel::<()>();
        let mut inbound = Inbound::new(
            Box::pin(frames),
            async move {
                let _ = ended.await;
                Ending::GoingAway
            },
            bounds(),
            None,
        );
        let started = Arc::clone(&inbound.started);
        for _ in 0..3 {
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
