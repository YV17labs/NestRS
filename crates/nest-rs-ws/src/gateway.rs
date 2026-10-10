use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use nest_rs_core::{Container, Correlation, RequestContinuation, RequestScope, operation_log};
use nest_rs_http::DetachedWork;
use nest_rs_pipes::PipeError;
use tracing::Instrument;

use poem::web::websocket::{CloseCode, Message, WebSocket, WebSocketConfig};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response};

use crate::WsReply;
use crate::config::WsConfig;
use crate::context::{BoxFuture, Captured, SocketContext};
use crate::envelope::WsEnvelope;
use crate::guard::EventLayerTable;
use crate::server::{ConnId, Registry, WsClient, WsServer};

/// Per-connection message dispatcher a gateway implements, emitted by
/// `#[messages]`. The gateway is a singleton; hooks take `&self` and the
/// connecting socket's [`WsClient`].
#[async_trait]
pub trait Gateway: Send + Sync + 'static {
    /// Route one decoded message to its handler and wrap the return in [`WsReply`].
    async fn dispatch(&self, client: &WsClient, event: &str, data: serde_json::Value) -> WsReply;

    /// Runs once when a socket connects, after the upgrade guards pass. One
    /// that panics leaves the connection set up halfway, so the socket is not
    /// served: it is closed with RFC 6455 §7.4.1's 1011 Internal Error.
    async fn on_connect(&self, client: &WsClient) {
        let _ = client;
    }

    /// Runs while the connection is still registered, so a hook can reach the
    /// leaving client's rooms before they are dropped — after a connect hook
    /// that panicked too, so what it set up can still be taken down.
    async fn on_disconnect(&self, client: &WsClient) {
        let _ = client;
    }
}

/// A per-message data-pipe runner with the container already captured, built
/// once when a gateway mounts.
pub type WsDataFold = dyn Fn(&str, &mut serde_json::Value) -> Result<(), PipeError> + Send + Sync;

/// Bridge slot for global pipes on a WS message's `data`, seeded by
/// `nest-rs-guards` (which depends on this crate, not the reverse) with a fold
/// of every [`GlobalPipe::transform_ws_data`](nest_rs_pipes::GlobalPipe).
pub struct WsDataPipe(pub fn(&Container, &str, &mut serde_json::Value) -> Result<(), PipeError>);

/// Resolve the [`WsDataPipe`] bridge at gateway mount into a runner with the
/// container captured; `None` when no global pipes are registered.
pub fn resolve_ws_data_pipe(container: &Container) -> Option<Arc<WsDataFold>> {
    let bridge = container.get::<WsDataPipe>()?;
    let container = container.clone();
    Some(Arc::new(
        move |event: &str, data: &mut serde_json::Value| (bridge.0)(&container, event, data),
    ))
}

/// Assemble a [`GatewayEndpoint`] from a gateway, its resolved per-connection
/// wiring, and the [`DetachedWork`] its `HttpEndpointMeta` declares. Called by
/// `#[messages]`-generated mount code.
pub fn gateway_endpoint<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: EventLayerTable,
    ctx: Option<Arc<dyn SocketContext>>,
    data_pipe: Option<Arc<WsDataFold>>,
    sockets: DetachedWork,
) -> GatewayEndpoint<G, N> {
    GatewayEndpoint {
        gateway,
        server,
        guards: Arc::new(guards),
        ctx,
        data_pipe,
        sockets,
    }
}

/// The endpoint returned by [`gateway_endpoint`], holding the gateway's own
/// [`WsServer<N>`].
pub struct GatewayEndpoint<G, N: 'static = crate::server::Global> {
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: Arc<EventLayerTable>,
    ctx: Option<Arc<dyn SocketContext>>,
    data_pipe: Option<Arc<WsDataFold>>,
    /// Every socket this gateway serves: an upgraded socket outlives the HTTP
    /// connection the shutdown window waits on, so the window waits on this too.
    sockets: DetachedWork,
}

impl<G: Gateway, N: 'static> Endpoint for GatewayEndpoint<G, N> {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        let (req, mut body) = req.split();
        let ws = WebSocket::from_request(&req, &mut body).await?;
        // The request does not survive into the connection task `on_upgrade` spawns.
        let ambient = self
            .ctx
            .as_ref()
            .map(|ctx| (ctx.clone(), ctx.capture(&req)));
        // `None` outside the HTTP request scope: `Scoped<T>` then answers `NoScope`.
        let root_container =
            nest_rs_http::current_request_scope().map(|scope| scope.root().clone());
        // Falls back to the bounded default, never to an unbounded lifetime or buffer.
        let ws_config = nest_rs_http::current_request_scope()
            .and_then(|scope| scope.root().get::<WsConfig>())
            .unwrap_or_default();
        let max_lifetime = ws_config.max_connection;
        let max_message_bytes = ws_config.max_message_bytes;
        // At the protocol layer, so tungstenite never buffers up to its 64 MiB default.
        let ws = ws.config(
            WebSocketConfig::default()
                .max_message_size(Some(max_message_bytes))
                .max_frame_size(Some(max_message_bytes)),
        );
        let gateway = Arc::clone(&self.gateway);
        let server = Arc::clone(&self.server);
        let guards = Arc::clone(&self.guards);
        let wiring = DispatchWiring {
            ambient,
            data_pipe: self.data_pipe.clone(),
            root_container,
            // The connection inherits the upgrade request's id and actor.
            connection: nest_rs_core::Correlation::inherited(),
        };
        let limits = SocketLimits {
            max_lifetime,
            max_message_bytes,
        };
        let sockets = self.sockets.clone();
        Ok(ws
            .on_upgrade(move |socket| {
                // Installed once for the whole task; a per-message install wins inside it.
                let connection = wiring.connection.clone();
                let going_away = sockets.going_away();
                async move {
                    let _ = sockets
                        .run(nest_rs_core::with_request_scope(
                            None,
                            connection,
                            serve_connection(
                                gateway, server, guards, wiring, limits, socket, going_away,
                            ),
                        ))
                        .await;
                }
            })
            .into_response())
    }
}

/// Per-connection dispatch wiring resolved once at upgrade and threaded into
/// every message.
struct DispatchWiring {
    ambient: Option<(Arc<dyn SocketContext>, Captured)>,
    data_pipe: Option<Arc<WsDataFold>>,
    root_container: Option<Container>,
    /// The upgrade that opened this socket, actor included: nothing
    /// re-authenticates per message.
    connection: nest_rs_core::Correlation,
}

/// Per-socket limits resolved once at upgrade from [`WsConfig`].
#[derive(Clone, Copy)]
struct SocketLimits {
    /// Socket-lifetime ceiling; `None` ⇒ unlimited.
    max_lifetime: Option<Duration>,
    /// Per-message byte cap (also enforced at the protocol layer).
    max_message_bytes: usize,
}

/// RAII cleanup for a connection's [`WsServer`] registry entry, so it cannot
/// outlive the connection task when gateway code panics and unwinds.
struct RegistryGuard<N: 'static> {
    server: Arc<WsServer<N>>,
    conn_id: ConnId,
}

impl<N: 'static> Drop for RegistryGuard<N> {
    fn drop(&mut self) {
        self.server.disconnect(self.conn_id);
    }
}

/// Drive one connection. Pushes funnel through one outbox drained by a writer
/// task, so the read loop never blocks on the `Sink`.
///
/// `going_away` is the transport's shutdown signal: the socket then closes with
/// 1001, after the message it is answering.
async fn serve_connection<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: Arc<EventLayerTable>,
    wiring: DispatchWiring,
    limits: SocketLimits,
    socket: poem::web::websocket::WebSocketStream,
    going_away: impl Future<Output = ()>,
) {
    let (mut sink, mut stream) = socket.split();
    let (outbox, mut rx) =
        tokio::sync::mpsc::channel::<crate::server::Frame>(crate::server::OUTBOX_CAPACITY);

    // The `Sink` comes back when the outbox closes, so the Close frame is written
    // after every reply already queued.
    let mut writer = Writer(tokio::spawn(nest_rs_core::panic::contain(async move {
        while let Some(frame) = rx.recv().await {
            if sink.send(Message::Text(frame.to_string())).await.is_err() {
                break;
            }
        }
        sink
    })));

    let conn_id = server.connect(outbox.clone());
    let registry_guard = RegistryGuard {
        server: Arc::clone(&server),
        conn_id,
    };
    let registry: Arc<dyn Registry> = server.clone();
    let client = WsClient::new(conn_id, registry);

    // Two short spans rather than one wrapping the loop: a parent open for the
    // socket's life would file every message under a span that never closes.
    let set_up = under_connection(
        &wiring.connection,
        Unit::Connect,
        conn_id,
        nest_rs_core::operation_span!(
            crate::unit::CONNECT,
            &wiring.connection,
            ws.connection_id = conn_id,
        ),
        gateway.on_connect(&client),
    )
    .await;

    // The lifetime ceiling bounds a principal captured once at the upgrade;
    // `None` is an inert `select!` arm.
    let mut lifetime = limits
        .max_lifetime
        .map(|ttl| Box::pin(tokio::time::sleep(ttl)));

    let mut going_away = std::pin::pin!(going_away);
    let closure = loop {
        if !set_up {
            break Closure::Server(CloseCode::Error, CONNECT_FAILED);
        }
        tokio::select! {
            // Shutdown wins over another read and over the ceiling.
            biased;
            () = &mut going_away => {
                // `debug`: shutdown is logged once; a line per socket is noise.
                tracing::debug!(
                    target: crate::TARGET,
                    conn_id,
                    close_code = u16::from(CloseCode::Away),
                    "closing socket: the server is going away",
                );
                break Closure::Server(CloseCode::Away, GOING_AWAY);
            }
            // The deadline is absolute (set at connect), so losing the race does not reset it.
            () = async {
                match lifetime.as_mut() {
                    Some(sleep) => sleep.as_mut().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                tracing::info!(
                    target: crate::TARGET,
                    conn_id,
                    close_code = u16::from(CloseCode::Away),
                    "closing socket: max lifetime reached",
                );
                break Closure::Server(CloseCode::Away, LIFETIME_REACHED);
            }
            message = stream.next() => {
                let Some(message) = message else { break Closure::PeerGone };
                match message {
                    Ok(Message::Text(text)) => {
                        // The boundary case the protocol-layer cap lets through; framing is
                        // intact, so the socket survives and the refusal goes in band.
                        if text.len() > limits.max_message_bytes {
                            let frame =
                                refuse_oversize(conn_id, text.len(), limits.max_message_bytes);
                            if outbox.try_send(frame.into()).is_err() {
                                break stalled_outbox(conn_id);
                            }
                            continue;
                        }
                        if let Some(reply) =
                            handle_text(&*gateway, &guards, &wiring, &client, &text).await
                        {
                            // The pushes' outbox, so ordering with broadcasts the handler
                            // triggered is preserved.
                            if outbox.try_send(reply.into()).is_err() {
                                break stalled_outbox(conn_id);
                            }
                        }
                    }
                    // RFC 6455 §7.4.1's 1003 is the other conformant answer; framing is
                    // intact, so the refusal goes in band.
                    Ok(Message::Binary(data)) => {
                        let frame = refuse_binary(conn_id, data.len());
                        if outbox.try_send(frame.into()).is_err() {
                            break stalled_outbox(conn_id);
                        }
                    }
                    Ok(Message::Close(_)) => break Closure::Echo,
                    // tungstenite queues the Pong before a Ping reaches this loop.
                    Ok(Message::Ping(_) | Message::Pong(_)) => {}
                    Err(err) => {
                        tracing::debug!(
                            target: crate::TARGET,
                            conn_id,
                            error = %nest_rs_core::error_message(&err),
                            close_code = u16::from(CloseCode::Error),
                            "websocket read error",
                        );
                        break Closure::Server(CloseCode::Error, READ_FAILED);
                    }
                }
            }
        }
    };

    // The registry guard drops before the writer is awaited, releasing the
    // registry's `Sender` clone so the writer observes the channel close.
    under_connection(
        &wiring.connection,
        Unit::Disconnect,
        conn_id,
        nest_rs_core::operation_span!(
            crate::unit::DISCONNECT,
            &wiring.connection,
            ws.connection_id = conn_id,
        ),
        gateway.on_disconnect(&client),
    )
    .await;
    drop(registry_guard);
    drop(outbox);
    // Bounded: a peer that stopped reading parks the writer; past the grace
    // `Writer`'s `Drop` aborts it.
    let closed = tokio::time::timeout(DetachedWork::CLOSE_GRACE, async {
        // Without the `Sink`, which went down with the task, the peer reads 1006
        // (RFC 6455 §7.4.1).
        if let Some(sink) = writer_ended((&mut writer.0).await, conn_id) {
            close_socket(sink, closure, conn_id).await;
        }
    })
    .await;
    if closed.is_err() {
        tracing::debug!(
            target: crate::TARGET,
            conn_id,
            close_grace_ms = u64::try_from(DetachedWork::CLOSE_GRACE.as_millis()).unwrap_or(u64::MAX),
            "socket dropped: the peer did not take its replies and close within the grace",
        );
    }
}

/// The task writing a connection's frames, aborted if the connection is
/// dropped before it is joined: it owns the sink, so it would keep the socket open.
struct Writer(tokio::task::JoinHandle<Written<WsSink>>);

impl Drop for Writer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What the writer ends with: the `Sink` back, or what it panicked with.
type Written<S> = Result<S, Box<dyn std::any::Any + Send>>;

/// The `Sink` the writer gave back, once its panic, if any, is said. The
/// writer is aborted only after this wait, so a `JoinError` here is its
/// runtime going down.
fn writer_ended<S>(joined: Result<Written<S>, tokio::task::JoinError>, conn_id: u64) -> Option<S> {
    match joined {
        Ok(Ok(sink)) => Some(sink),
        Ok(Err(payload)) => {
            nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload.as_ref(),
                "writer task panicked",
                conn_id,
            );
            None
        }
        Err(_) => None,
    }
}

/// The write half of a connection's socket.
type WsSink = futures_util::stream::SplitSink<poem::web::websocket::WebSocketStream, Message>;

/// Close a socket whose peer stopped draining its full outbox.
fn stalled_outbox(conn_id: u64) -> Closure {
    tracing::warn!(
        target: crate::TARGET,
        conn_id,
        close_code = u16::from(CloseCode::Policy),
        "closing socket: the peer stopped draining and the outbox is full",
    );
    Closure::Server(CloseCode::Policy, OUTBOX_STALLED)
}

/// Why the socket is ending, and — when the server is the one ending it — the
/// RFC 6455 §7.4.1 status code the peer is told it in, so a deliberate close is
/// never read as 1006, a network fault.
enum Closure {
    /// The peer sent a Close (§5.5.1). tungstenite has already queued the echo,
    /// so this owes only the flush; a second Close is refused (`SendAfterClosing`).
    Echo,
    /// The stream ended without a Close frame: the peer is gone, and 1006 is
    /// §7.4.1's code for exactly this.
    PeerGone,
    /// The server ended it, under the code §7.4.1 defines for the cause.
    Server(CloseCode, &'static str),
}

/// §7.4.1 **1001 Going Away**, not 1008: 1008 is for a received message that
/// violates policy, and a clock ended this one. A client answers 1001 by reconnecting.
const LIFETIME_REACHED: &str = "connection lifetime reached, re-upgrade to continue";

/// §7.4.1 **1001 Going Away**, its code for "a server going down".
const GOING_AWAY: &str = "the server is going away, reconnect to continue";

/// §7.4.1 **1011 Internal Error**: the connect hook unwound.
const CONNECT_FAILED: &str = "the connection could not be set up";

/// §7.4.1 **1008 Policy Violation**, its generic code: a peer that will not
/// drain a bounded outbox is shed.
const OUTBOX_STALLED: &str = "outbox full, the client is not draining its messages";

/// §7.4.1 **1011 Internal Error**, not 1009: poem stringifies tungstenite's
/// errors into an opaque `io::Error`, so the message cap cannot be told from a
/// framing fault.
const READ_FAILED: &str = "the connection could not be read";

/// Put the Close frame on the wire, then flush — the flush also drives out the
/// echo queued for [`Closure::Echo`]. Best-effort: a gone peer is logged at `debug`.
async fn close_socket(mut sink: WsSink, closure: Closure, conn_id: ConnId) {
    if let Closure::Server(code, reason) = closure
        && let Err(err) = sink
            .send(Message::Close(Some((code, reason.to_string()))))
            .await
    {
        tracing::debug!(
            target: crate::TARGET,
            conn_id,
            error = %nest_rs_core::error_message(&err),
            "websocket close frame undelivered",
        );
        return;
    }
    if let Err(err) = SinkExt::close(&mut sink).await {
        tracing::debug!(
            target: crate::TARGET,
            conn_id,
            error = %nest_rs_core::error_message(&err),
            "websocket close handshake unfinished",
        );
    }
}

/// Refuse a message past the per-message cap, telling the client and, at
/// `debug`, the operator.
fn refuse_oversize(conn_id: ConnId, bytes: usize, max_message_bytes: usize) -> String {
    tracing::debug!(
        target: crate::TARGET,
        conn_id,
        bytes,
        max_message_bytes,
        "websocket message refused: over the per-message cap",
    );
    error_frame("error", &crate::WsError::new("message too large"))
}

/// Refuse a Binary frame (RFC 6455 §5.6), which carries no JSON envelope,
/// telling the client and, at `debug`, the operator.
fn refuse_binary(conn_id: ConnId, bytes: usize) -> String {
    tracing::debug!(
        target: crate::TARGET,
        conn_id,
        bytes,
        "websocket message refused: binary frames carry no envelope",
    );
    error_frame(
        "error",
        &crate::WsError::new("binary frames are not supported"),
    )
}

/// Run one connection-lifecycle hook under the connection's identity, and say
/// whether it ran to its end: `false` when it unwound (contained here, filed as
/// `panic`).
///
/// The span is passed in because `tracing` fixes a span's name at the macro.
async fn under_connection<F: Future<Output = ()>>(
    connection: &Correlation,
    unit: Unit<'_>,
    conn_id: ConnId,
    span: tracing::Span,
    hook: F,
) -> bool {
    let line = UnitLine::open(unit, conn_id, connection.clone(), span.clone());
    let ran = nest_rs_core::with_request_scope(
        None,
        connection.clone(),
        nest_rs_core::panic::contain(hook),
    )
    .instrument(span)
    .await;
    match ran {
        Ok(()) => {
            line.file(operation_log::OK);
            true
        }
        Err(payload) => {
            line.unwound(&*payload);
            false
        }
    }
}

/// The units a socket carries, each filed under its own canonical name.
#[derive(Clone, Copy)]
enum Unit<'a> {
    /// One message, named by its event.
    Message { event: &'a str },
    /// The connect hook.
    Connect,
    /// The disconnect hook.
    Disconnect,
}

/// One unit's line, filed exactly once — by the end the connection loop saw,
/// or by `Drop` as [`CANCELLED`](operation_log::CANCELLED) when the transport
/// stopped the socket with this unit still running.
///
/// One macro call per unit: `name:` must be a constant at each call site.
struct UnitLine<'a> {
    unit: Unit<'a>,
    conn_id: ConnId,
    /// Entered to file the line: a `Drop` runs while the unit's future is torn
    /// down, not reliably inside the scope that future installed.
    correlation: Correlation,
    /// The unit's span, which records the outcome the line files.
    span: tracing::Span,
    started: std::time::Instant,
    filed: bool,
}

impl<'a> UnitLine<'a> {
    fn open(
        unit: Unit<'a>,
        conn_id: ConnId,
        correlation: Correlation,
        span: tracing::Span,
    ) -> Self {
        Self {
            unit,
            conn_id,
            correlation,
            span,
            started: std::time::Instant::now(),
            filed: false,
        }
    }

    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
    }

    /// The unit unwound: file `panic`, and give the operator the panic's text,
    /// which the client is never told.
    fn unwound(mut self, payload: &(dyn std::any::Any + Send)) {
        self.emit(operation_log::PANIC);
        let conn_id = self.conn_id;
        RequestContinuation::new(None, self.correlation.clone()).enter(|| match self.unit {
            Unit::Message { event } => nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "websocket handler panicked; its client is answered with an internal error",
                conn_id,
                event,
            ),
            Unit::Connect => nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "websocket connect hook panicked; the socket is closed unserved",
                conn_id,
                close_code = u16::from(CloseCode::Error),
            ),
            Unit::Disconnect => nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "websocket disconnect hook panicked; the close goes on",
                conn_id,
            ),
        });
    }

    fn emit(&mut self, outcome: &'static str) {
        self.filed = true;
        let conn_id = self.conn_id;
        let span = &self.span;
        let started = self.started;
        RequestContinuation::new(None, self.correlation.clone()).enter(|| match self.unit {
            Unit::Message { event } => nest_rs_core::operation_line!(
                crate::unit::MESSAGE,
                span: span,
                outcome: outcome,
                started: started,
                event,
                conn_id,
            ),
            Unit::Connect => nest_rs_core::operation_line!(
                crate::unit::CONNECT,
                span: span,
                outcome: outcome,
                started: started,
                conn_id,
            ),
            Unit::Disconnect => nest_rs_core::operation_line!(
                crate::unit::DISCONNECT,
                span: span,
                outcome: outcome,
                started: started,
                conn_id,
            ),
        });
    }
}

impl Drop for UnitLine<'_> {
    fn drop(&mut self) {
        if !self.filed {
            self.emit(if std::thread::panicking() {
                operation_log::PANIC
            } else {
                operation_log::CANCELLED
            });
        }
    }
}

/// Decode, guard and dispatch one text message. Per-message guards run
/// **inside** [`SocketContext::around`], so they see the ambient task-locals
/// (`current_ability()`) the handler does.
async fn handle_text<G: Gateway>(
    gateway: &G,
    guards: &EventLayerTable,
    wiring: &DispatchWiring,
    client: &WsClient,
    text: &str,
) -> Option<String> {
    let envelope: WsEnvelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(_) => {
            return Some(error_frame(
                "error",
                &crate::WsError::new("invalid envelope"),
            ));
        }
    };
    let WsEnvelope { event, mut data } = envelope;
    let data_pipe = wiring.data_pipe.as_ref();
    let event_ref = event.clone();
    let conn_id = client.id();
    let inner: BoxFuture<'_, WsReply> = Box::pin(async move {
        // The guard bridge filed the denial while it still held it.
        if let Err(frame) = guards.check(client, &event_ref, &data).await {
            return WsReply::Error(frame);
        }
        // Guards see the raw value; global data pipes run after them.
        if let Some(pipe) = data_pipe
            && let Err(err) = pipe(&event_ref, &mut data)
        {
            return WsReply::pipe_error(&event_ref, "data", err);
        }
        gateway.dispatch(client, &event_ref, data).await
    });
    let dispatch: BoxFuture<'_, WsReply> = match wiring.ambient.as_ref() {
        Some((ctx, captured)) => ctx.around(captured, inner),
        None => inner,
    };
    // The span declares the `actor_id` field the ability guard records into.
    let correlation = wiring.connection.child();
    let span = nest_rs_core::operation_span!(
        crate::unit::MESSAGE,
        &correlation,
        ws.event = %event,
        ws.connection_id = conn_id,
    );
    // The correlation is installed even without a container; only `Scoped<T>` goes without.
    let scope = wiring
        .root_container
        .as_ref()
        .map(|container| Arc::new(RequestScope::new(container.clone())));
    let line = UnitLine::open(
        Unit::Message { event: &event },
        conn_id,
        correlation.clone(),
        span.clone(),
    );
    // A panic is contained at the message, so the socket goes on serving.
    let ran = nest_rs_core::with_request_scope(
        scope,
        correlation,
        nest_rs_core::panic::contain(dispatch),
    )
    .instrument(span)
    .await;
    let reply = match ran {
        Ok(reply) => {
            line.file(match &reply {
                WsReply::Error(_) => operation_log::ERROR,
                _ => operation_log::OK,
            });
            reply
        }
        Err(payload) => {
            line.unwound(&*payload);
            WsReply::Error(crate::WsError::new(nest_rs_core::OPAQUE_CLIENT_MESSAGE))
        }
    };
    match reply {
        WsReply::Reply(data) => {
            let envelope = WsEnvelope { event, data };
            match serde_json::to_string(&envelope) {
                Ok(frame) => Some(frame),
                Err(err) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        event = %envelope.event,
                        error = %nest_rs_core::error_message(&err),
                        "failed to serialize reply",
                    );
                    Some(error_frame(
                        &envelope.event,
                        &crate::WsError::new(nest_rs_core::OPAQUE_CLIENT_MESSAGE),
                    ))
                }
            }
        }
        WsReply::None => None,
        WsReply::Error(error) => Some(error_frame(&event, &error)),
    }
}

fn error_frame(event: &str, error: &crate::WsError) -> String {
    WsEnvelope::encode(event, error)
        .unwrap_or_else(|_| String::from(r#"{"event":"error","data":{"error":"internal"}}"#))
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;
    use crate::guard::WsMessageCheck;

    #[tokio::test]
    async fn a_writer_that_panicked_is_said_once_without_its_value() {
        let logs = nest_rs_testing::LogCapture::install();
        let joined = tokio::spawn(nest_rs_core::panic::contain(async {
            serde_json::from_str::<u64>(r#""sk_live_51HsecretTOKEN""#).unwrap()
        }))
        .await;

        assert_eq!(writer_ended(joined, 7), None);
        let line = logs.expect_one(crate::TARGET, "writer task panicked");
        assert_eq!(line.level, "error");
        assert_eq!(line.field("conn_id").as_deref(), Some("7"));
        let said = line.field(nest_rs_core::panic::FIELD).unwrap_or_default();
        assert!(
            said.contains("invalid type: a string, expected u64") && !said.contains("sk_live"),
            "{line:#?}"
        );
    }

    #[tokio::test]
    async fn a_connection_hook_runs_under_the_connections_identity() {
        let logs = nest_rs_testing::LogCapture::install();
        let connection = nest_rs_core::Correlation::minted(None);
        let trace_id = connection.trace_id().to_hex();

        under_connection(
            &connection,
            Unit::Connect,
            7,
            nest_rs_core::operation_span!(
                crate::unit::CONNECT,
                &connection,
                ws.connection_id = 7u64,
            ),
            async {
                tracing::info!(
                    target: crate::TARGET,
                    ambient = nest_rs_core::current_trace_id().map(|id| id.to_hex()),
                    span = tracing::Span::current().metadata().map(|meta| meta.name()),
                    "hook ran",
                );
            },
        )
        .await;

        let event = logs.expect_one("nest_rs::ws", "hook ran");
        assert_eq!(
            event.field("ambient").as_deref(),
            Some(trace_id.as_str()),
            "`current_trace_id()` inside the hook is the connection's: {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("span").as_deref(),
            Some(crate::unit::CONNECT.name()),
            "the hook's events are rooted at the connection span: {:?}",
            event.fields,
        );

        let opened = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::CONNECT.name(),
        );
        assert_eq!(opened.message, crate::unit::CONNECT.name());
        assert_eq!(opened.field("conn_id").as_deref(), Some("7"));
        assert!(opened.field("duration_ms").is_some());
    }

    #[tokio::test]
    async fn a_message_over_the_cap_is_refused_to_the_client_and_recorded_for_the_operator() {
        let logs = nest_rs_testing::LogCapture::install();
        let connection = nest_rs_core::Correlation::minted(None);
        let trace_id = connection.trace_id();

        let frame = nest_rs_core::with_request_scope(None, connection, async {
            let joined = nest_rs_core::current_trace_id();
            assert_eq!(
                joined,
                Some(trace_id),
                "the refusal joins the connection's conversation",
            );
            refuse_oversize(7, 4096, 1024)
        })
        .await;

        assert!(
            frame.contains("message too large"),
            "the client is told why: {frame}",
        );
        let event = logs.expect_one(
            "nest_rs::ws",
            "websocket message refused: over the per-message cap",
        );
        assert!(
            event.field("trace_id").is_none(),
            "the correlation is the line's, not the event's: {:?}",
            event.fields,
        );
        assert_eq!(event.field("conn_id").as_deref(), Some("7"));
        assert_eq!(event.field("bytes").as_deref(), Some("4096"));
        assert_eq!(event.field("max_message_bytes").as_deref(), Some("1024"));
    }

    struct DenyAll;

    #[async_trait]
    impl WsMessageCheck for DenyAll {
        async fn check(
            &self,
            _client: &WsClient,
            _event: &str,
            _data: &serde_json::Value,
        ) -> Result<(), crate::WsError> {
            Err(crate::WsError::with_details(
                "author `banned` is not allowed to post",
                serde_json::json!({ "reason": "forbidden" }),
            ))
        }

        fn type_key(&self) -> TypeId {
            TypeId::of::<Self>()
        }
    }

    struct Echo;

    #[async_trait]
    impl Gateway for Echo {
        async fn dispatch(
            &self,
            _client: &WsClient,
            _event: &str,
            data: serde_json::Value,
        ) -> WsReply {
            WsReply::Reply(data)
        }
    }

    fn wiring() -> DispatchWiring {
        DispatchWiring {
            ambient: None,
            data_pipe: None,
            root_container: None,
            connection: nest_rs_core::Correlation::minted(None),
        }
    }

    #[tokio::test]
    async fn a_denied_message_answers_the_frame_its_check_rendered_and_files_nothing_more() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut guards = EventLayerTable::new();
        guards.insert("moderated", vec![Arc::new(DenyAll)]);

        let frame = handle_text(
            &Echo,
            &guards,
            &wiring(),
            &WsClient::for_test(),
            r#"{"event":"moderated","data":"hi"}"#,
        )
        .await
        .expect("a denial replies with an error frame");

        let frame: serde_json::Value = serde_json::from_str(&frame).expect("a JSON frame");
        assert_eq!(frame["event"], "moderated");
        assert_eq!(
            frame["data"]["error"],
            "author `banned` is not allowed to post"
        );
        assert_eq!(
            frame["data"]["errors"]["reason"], "forbidden",
            "the structured half the check rendered travels whole: {frame}",
        );
        assert!(
            logs.events()
                .iter()
                .all(|event| event.target != crate::TARGET || event.level != "warn"),
            "the check filed the denial where it held it; the gateway adds no second line: {:#?}",
            logs.events(),
        );
    }

    #[tokio::test]
    async fn an_allowed_message_logs_no_denial() {
        let logs = nest_rs_testing::LogCapture::install();
        let frame = handle_text(
            &Echo,
            &EventLayerTable::new(),
            &wiring(),
            &WsClient::for_test(),
            r#"{"event":"open","data":"hi"}"#,
        )
        .await
        .expect("an echo reply");
        assert!(frame.contains("hi"), "{frame}");
        assert!(
            logs.events().iter().all(|event| event.level != "warn"),
            "an answered message files no warning: {:#?}",
            logs.events(),
        );
    }

    /// Pins the event, not the path: every `try_send` failure routes through
    /// `stalled_outbox`.
    #[test]
    fn a_peer_that_stops_draining_is_closed_with_a_reason() {
        let logs = nest_rs_testing::LogCapture::install();
        let closure = stalled_outbox(7);

        assert!(matches!(
            closure,
            Closure::Server(CloseCode::Policy, OUTBOX_STALLED)
        ));
        let event = logs.expect_one(
            crate::TARGET,
            "closing socket: the peer stopped draining and the outbox is full",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("conn_id").as_deref(), Some("7"));
        assert_eq!(
            event.field("close_code").as_deref(),
            Some(u16::from(CloseCode::Policy).to_string().as_str()),
            "the code the peer is actually sent, per RFC 6455 §7.4.1",
        );
    }
}
