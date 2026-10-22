use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::FutureExt as _;
use futures_util::future::Either;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use nest_rs_core::__private::{ClassGate, RequestScopes};
use nest_rs_core::{
    Container, Continuation, Correlation, Handler, RequestContinuation, operation_log,
};
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_guards::GuardSpec;
use nest_rs_http::__private::websocket::{
    self, CloseCode, Message, WebSocketLimits, WebSocketUpgrade,
};
use nest_rs_http::{BoxEndpoint, DetachedWork, Endpoint, Request, Response};
use nest_rs_pipes::PipeSpec;
use tracing::Instrument;

use crate::WsReply;
use crate::config::WsConfig;
use crate::envelope::{HandlerFailed, WsEnvelope};
use crate::message::{Answered, DeclaredMessage, MessageTable};
use crate::server::{ConnId, Registry, WsClient, WsServer};

/// Per-connection message dispatcher a gateway implements, emitted by
/// `#[messages]`. The gateway is a singleton; hooks take `&self` and the
/// connecting socket's [`WsClient`].
#[async_trait]
pub trait Gateway: Send + Sync + 'static {
    /// Run the handler `event` names on `data`: its parameters, its payload's
    /// pipes, the call and the posture's mask — what a message's unit runs
    /// last, once its guards and its layers let it. An error comes back
    /// rendered, its line not yet filed.
    #[doc(hidden)]
    async fn __nestrs_handle(
        &self,
        client: &WsClient,
        event: &str,
        data: serde_json::Value,
    ) -> Result<WsReply, HandlerFailed>;

    /// The class gate the posture of the handler `event` names declares, with
    /// that handler: `None` for a `#[public]` handler, or an event none serves.
    #[doc(hidden)]
    fn __nestrs_gate(&self, event: &str) -> Option<(&'static Handler, ClassGate)>;

    /// Call the handler `event` names on `data`, as a test calls it on its
    /// own: the posture's class gate, its parameters, its payload's pipes, the
    /// call and the posture's mask, a refusal or an error answered as its
    /// frame and filed at once. No guard and no layer runs here — a message
    /// arriving on a socket runs them around the handler, the class gate after
    /// its guards.
    async fn dispatch(&self, client: &WsClient, event: &str, data: serde_json::Value) -> WsReply {
        let data = match self.__nestrs_gate(event) {
            Some((handler, gate)) => match crate::guard::gate_alone(client, handler, gate, data) {
                Ok(data) => data,
                Err(refused) => return WsReply::Error(refused),
            },
            None => data,
        };
        match self.__nestrs_handle(client, event, data).await {
            Ok(reply) => reply,
            Err(failed) => failed.into_reply(event),
        }
    }

    /// Runs once when a socket connects, after the upgrade guards pass. One
    /// that panics, or outlasts its deadline, leaves the connection set up
    /// halfway, so the socket is not served: it is closed with RFC 6455
    /// §7.4.1's 1011 Internal Error.
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

/// The endpoint a gateway's upgrade mounts at, assembled from the gateway, its
/// messages, the container it mounts from, and the [`DetachedWork`] its
/// `HttpEndpointMeta` declares; what each connection needs from the container
/// is resolved here, once. Called by `#[messages]`-generated mount code.
pub fn gateway_endpoint<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    messages: MessageTable,
    container: &Container,
    sockets: DetachedWork,
) -> BoxEndpoint {
    BoxEndpoint::new(GatewayEndpoint {
        gateway,
        server,
        messages: Arc::new(messages),
        unit_of_work: container.get_dyn::<dyn UnitOfWork>(),
        // Falls back to the bounded default, never to an unbounded lifetime or buffer.
        config: container.get::<WsConfig>().unwrap_or_default(),
        scopes: RequestScopes::new(container.clone()),
        sockets,
    })
}

/// The wiring `#[messages]` attaches for the gateway at `path`: its
/// `upgrade` guards are phase-validated and resolvable, and each of its
/// `messages` composes over its `host_pipes` — a layer no imported module
/// provides, or two `#[meta]` values of one type at one level, fail the boot
/// naming the site. A unit deadline the shutdown window cannot hold is said at
/// `warn`.
pub fn wire_gateway(
    container: &Container,
    path: &str,
    upgrade: &[GuardSpec],
    host_pipes: &[PipeSpec],
    messages: &[DeclaredMessage],
) -> nest_rs_core::anyhow::Result<()> {
    crate::guard::validate_upgrade(container, path, upgrade)?;
    crate::message::compose_declared(container, host_pipes, messages)?;
    let window = container
        .get::<nest_rs_http::HttpConfig>()
        .unwrap_or_default()
        .shutdown_timeout;
    container
        .get::<WsConfig>()
        .unwrap_or_default()
        .warn_when_units_outlast_the_window(window, path);
    Ok(())
}

/// The endpoint `#[messages]` mounts a gateway's upgrade at, holding the
/// gateway's own [`WsServer<N>`].
struct GatewayEndpoint<G, N: 'static> {
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    messages: Arc<MessageTable>,
    /// The binding each message and connection hook runs its unit of work
    /// through; `None` runs them bare.
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    config: Arc<WsConfig>,
    /// Where each message gets its request scope.
    scopes: RequestScopes,
    /// Every socket this gateway serves: an upgraded socket outlives the HTTP
    /// connection the shutdown window waits on, so the window waits on this too.
    sockets: DetachedWork,
}

impl<G: Gateway, N: 'static> Endpoint for GatewayEndpoint<G, N> {
    async fn call(&self, mut req: Request) -> nest_rs_http::Result<Response> {
        // The transport checked the origin and ran the upgrade's guards in front.
        let upgrade = WebSocketUpgrade::from_request(&mut req)?;
        let max_lifetime = self.config.max_connection;
        let max_message_bytes = self.config.max_message_bytes;
        let gateway = Arc::clone(&self.gateway);
        let server = Arc::clone(&self.server);
        let messages = Arc::clone(&self.messages);
        // What the upgrade carries into its connection — what its guards
        // attached included: never its scope, never its transaction.
        let carried = RequestContinuation::capture(Continuation::Connection);
        let wiring = DispatchWiring {
            unit_of_work: self.unit_of_work.clone(),
            scopes: self.scopes.clone(),
            // The connection inherits the upgrade request's id and actor.
            connection: nest_rs_core::Correlation::inherited(),
            message_timeout: self.config.message_timeout,
            connect_timeout: self.config.connect_timeout,
            disconnect_timeout: self.config.disconnect_timeout,
        };
        let limits = SocketLimits {
            max_lifetime,
            max_message_bytes,
        };
        let sockets = self.sockets.clone();
        // At the protocol layer, so tungstenite never buffers past the cap.
        let protocol_limits = WebSocketLimits::new(max_message_bytes);
        Ok(upgrade.accept(None, protocol_limits, move |socket| {
            // Installed once for the whole task; each hook and message is a
            // unit nested in it, so it carries what the connection carries.
            let connection = wiring.connection.clone();
            let going_away = sockets.going_away();
            async move {
                let serving = serve_connection(
                    gateway, server, messages, wiring, limits, socket, going_away,
                );
                let serving = match carried {
                    Some(carried) => Either::Left(carried.scope(serving)),
                    None => {
                        Either::Right(nest_rs_core::with_request_scope(None, connection, serving))
                    }
                };
                let _ = sockets.run(serving).await;
            }
        }))
    }
}

/// Per-connection dispatch wiring resolved once at mount and threaded into
/// every message.
struct DispatchWiring {
    unit_of_work: Option<Arc<dyn UnitOfWork>>,
    scopes: RequestScopes,
    /// The upgrade that opened this socket, actor included: nothing
    /// re-authenticates per message.
    connection: nest_rs_core::Correlation,
    /// What one message may hold the socket for; `None` ⇒ no deadline.
    message_timeout: Option<Duration>,
    /// What the connect hook may hold the socket for; `None` ⇒ no deadline.
    connect_timeout: Option<Duration>,
    /// What the disconnect hook may hold the close for; `None` ⇒ no deadline.
    disconnect_timeout: Option<Duration>,
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
async fn serve_connection<G, N, S>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    messages: Arc<MessageTable>,
    wiring: DispatchWiring,
    limits: SocketLimits,
    socket: S,
    going_away: impl Future<Output = ()>,
) where
    G: Gateway,
    N: 'static,
    S: Stream<Item = Result<Message, websocket::Error>>
        + Sink<Message, Error = websocket::Error>
        + Send
        + 'static,
{
    let (mut sink, mut stream) = socket.split();
    let (outbox, mut rx) =
        tokio::sync::mpsc::channel::<crate::server::Frame>(crate::server::OUTBOX_CAPACITY);
    let conn_id = server.connect(outbox.clone());

    // The `Sink` comes back when the outbox closes, so the Close frame is written
    // after every reply already queued. A frame that cannot be written ends the
    // socket: a peer that stops reading sends nothing either.
    let (writer_ends, mut writer_stopped) = tokio::sync::oneshot::channel::<()>();
    let mut writer = Writer(tokio::spawn(nest_rs_core::panic::contain(async move {
        while let Some(frame) = rx.recv().await {
            if let Err(error) = sink.send(Message::Text(frame)).await {
                stopped_writing(conn_id, &error);
                break;
            }
        }
        drop(writer_ends);
        sink
    })));

    let registry_guard = RegistryGuard {
        server: Arc::clone(&server),
        conn_id,
    };
    let registry: Arc<dyn Registry> = server.clone();
    let client = WsClient::new(conn_id, registry);

    // Two short spans rather than one wrapping the loop: a parent open for the
    // socket's life would file every message under a span that never closes.
    let set_up = under_connection(
        &wiring,
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
            // Dropped as the writer ends: nothing more reaches the peer.
            _ = &mut writer_stopped => break Closure::WriterStopped,
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
                            handle_text(&*gateway, &messages, &wiring, &client, text.as_str()).await
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
                    // tungstenite queues the Pong before a Ping reaches this loop,
                    // and a raw frame is never read.
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                    Err(err) => {
                        let (code, reason) = websocket::read_failed(&err);
                        tracing::debug!(
                            target: crate::TARGET,
                            conn_id,
                            error = %nest_rs_core::error_message(&err),
                            close_code = u16::from(code),
                            "websocket read error",
                        );
                        break Closure::Server(code, reason);
                    }
                }
            }
        }
    };

    // The registry guard drops before the writer is awaited, releasing the
    // registry's `Sender` clone so the writer observes the channel close.
    under_connection(
        &wiring,
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
struct Writer<S>(tokio::task::JoinHandle<Written<WsSink<S>>>);

impl<S> Drop for Writer<S> {
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
type WsSink<S> = futures_util::stream::SplitSink<S, Message>;

/// The writer could not put a frame on the wire, and the socket ends with it.
fn stopped_writing(conn_id: ConnId, error: &websocket::Error) {
    if matches!(error, websocket::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut) {
        tracing::warn!(
            target: crate::TARGET,
            conn_id,
            error = %nest_rs_core::error_message(error),
            "the peer took nothing within the send deadline; it is cut off",
        );
    } else {
        tracing::debug!(
            target: crate::TARGET,
            conn_id,
            error = %nest_rs_core::error_message(error),
            "closing socket: a frame could not be written",
        );
    }
}

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
    /// The writer stopped on a frame it could not write — past the send
    /// deadline, most often — so a Close frame would not get through either:
    /// the peer reads 1006.
    WriterStopped,
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

/// Put the Close frame on the wire, then flush — the flush also drives out the
/// echo queued for [`Closure::Echo`]. Best-effort: a gone peer is logged at `debug`.
async fn close_socket<S>(mut sink: WsSink<S>, closure: Closure, conn_id: ConnId)
where
    S: Sink<Message, Error = websocket::Error>,
{
    if let Closure::WriterStopped = closure {
        return;
    }
    if let Closure::Server(code, reason) = closure
        && let Err(err) = sink.send(websocket::close_frame(code, reason)).await
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

/// Run one connection-lifecycle hook under the connection's identity, as a
/// unit of work of its own cut at `timeout`, and say whether it ran to its end:
/// `false` when it unwound (contained here, filed as `panic`), when it was cut
/// at its deadline, or when what it wrote could not be committed.
///
/// The span is passed in because `tracing` fixes a span's name at the macro.
async fn under_connection<F: Future<Output = ()> + Send>(
    wiring: &DispatchWiring,
    unit: Unit<'_>,
    conn_id: ConnId,
    span: tracing::Span,
    hook: F,
) -> bool {
    let connection = &wiring.connection;
    let line = UnitLine::open(unit, conn_id, connection.clone(), span.clone());
    let timeout = match unit {
        Unit::Connect => wiring.connect_timeout,
        Unit::Disconnect => wiring.disconnect_timeout,
        Unit::Message { .. } => wiring.message_timeout,
    };
    let deadline = timeout.map(|timeout| (timeout, tokio::time::Instant::now() + timeout));
    let spec = unit_spec(unit, deadline.map(|(_, at)| at));
    let body = nest_rs_core::panic::contain(hook).map(HookEnd::from);
    // The body alone is cut: the unit settles outside it, so a commit never is.
    let cut = async move {
        match deadline {
            Some((timeout, at)) => tokio::time::timeout_at(at, body)
                .await
                .unwrap_or(HookEnd::TimedOut(timeout)),
            None => body.await,
        }
    };
    let ran = nest_rs_core::with_request_scope(
        None,
        connection.clone(),
        run_unit(
            wiring.unit_of_work.as_ref(),
            spec,
            cut,
            |end| matches!(end, HookEnd::Ran),
            HookEnd::Unhonoured,
        ),
    )
    .instrument(span)
    .await;
    match ran {
        HookEnd::Ran => {
            line.file(operation_log::OK);
            true
        }
        HookEnd::Unwound(payload) => {
            line.unwound(&*payload);
            false
        }
        HookEnd::TimedOut(timeout) => {
            line.file(operation_log::ERROR);
            tracing::warn!(
                target: crate::TARGET,
                conn_id,
                unit = unit.declared().name(),
                timeout_secs = timeout.as_secs(),
                "websocket connection hook not finished within its deadline; it is cut",
            );
            false
        }
        HookEnd::Unhonoured(why) => {
            line.file(operation_log::ERROR);
            tracing::error!(
                target: crate::TARGET,
                conn_id,
                retryable = why.retryable,
                reason = why.reason,
                "websocket connection hook ran, but what it wrote could not be committed",
            );
            false
        }
    }
}

/// The unit of work one of a socket's units opens: one transaction, under the
/// request's executor scope, its body cut at `deadline`.
fn unit_spec(unit: Unit<'_>, deadline: Option<tokio::time::Instant>) -> UnitSpec {
    let spec = UnitSpec::new(
        unit.declared(),
        ExecutorScope::Request,
        TransactionPolicy::PerUnit,
    );
    match deadline {
        Some(at) => spec.with_deadline(at),
        None => spec,
    }
}

/// How a connection hook's unit ended.
enum HookEnd {
    /// It ran to its end, and what it wrote stands.
    Ran,
    /// It unwound, with what it panicked with.
    Unwound(Box<dyn std::any::Any + Send>),
    /// It outlasted its deadline, and was cut; what it wrote rolls back.
    TimedOut(Duration),
    /// It ran to its end, and its unit of work could not honour it.
    Unhonoured(nest_rs_database::Unhonoured),
}

impl From<std::thread::Result<()>> for HookEnd {
    fn from(ran: std::thread::Result<()>) -> Self {
        match ran {
            Ok(()) => Self::Ran,
            Err(payload) => Self::Unwound(payload),
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

impl Unit<'_> {
    /// The unit this edge declared for it.
    const fn declared(self) -> nest_rs_core::operation_log::Unit {
        match self {
            Self::Message { .. } => crate::unit::MESSAGE,
            Self::Connect => crate::unit::CONNECT,
            Self::Disconnect => crate::unit::DISCONNECT,
        }
    }
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

/// Decode one text message and answer it as one unit of work: its site's
/// chains around its handler, inside the unit of work it runs, cut at its
/// deadline — the guards inside the unit, so they see the ambient values the
/// handler does, and a denied message opens no transaction.
async fn handle_text<G: Gateway>(
    gateway: &G,
    messages: &MessageTable,
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
    let WsEnvelope { event, data } = envelope;
    let served = messages.get(&event);
    // What the lines and the span name: the event a handler declares, never
    // one only the client named.
    let named = served.map_or(operation_log::OTHER, |(event, _)| event);
    let conn_id = client.id();
    let deadline = wiring
        .message_timeout
        .map(|timeout| (timeout, tokio::time::Instant::now() + timeout));
    let body = async move {
        match served {
            Some((event, Some(message))) => {
                crate::message::answer(
                    gateway,
                    message,
                    client,
                    event,
                    data,
                    deadline.map(|(_, at)| at),
                )
                .await
            }
            Some((_, None)) => crate::message::refused(),
            None => Answered::failed(WsReply::unknown()),
        }
    };
    // The body alone is cut: the unit settles outside it, so a commit never is.
    let cut = async move {
        match deadline {
            Some((timeout, at)) => tokio::time::timeout_at(at, body)
                .await
                .unwrap_or_else(|_| timed_out(conn_id, named, timeout)),
            None => body.await,
        }
    };
    let spec = unit_spec(Unit::Message { event: named }, deadline.map(|(_, at)| at));
    let unit = run_unit(
        wiring.unit_of_work.as_ref(),
        spec,
        cut,
        |answered| !answered.failed,
        |_| Answered::opaque(),
    );
    // The span declares the `actor_id` field the ability guard records into.
    let correlation = wiring.connection.child();
    let span = nest_rs_core::operation_span!(
        crate::unit::MESSAGE,
        &correlation,
        ws.event = %named,
        ws.connection_id = conn_id,
    );
    let line = UnitLine::open(
        Unit::Message { event: named },
        conn_id,
        correlation.clone(),
        span.clone(),
    );
    // A panic is contained at the message, so the socket goes on serving.
    let ran = nest_rs_core::with_request_scope(
        Some(wiring.scopes.open()),
        correlation,
        nest_rs_core::panic::contain(unit),
    )
    .instrument(span)
    .await;
    let reply = match ran {
        Ok(answered) => {
            line.file(if answered.failed {
                operation_log::ERROR
            } else {
                operation_log::OK
            });
            answered.reply
        }
        Err(payload) => {
            line.unwound(&*payload);
            WsReply::Error(crate::WsError::internal())
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
                        event = %named,
                        error = %nest_rs_core::error_message(&err),
                        "failed to serialize reply",
                    );
                    Some(error_frame(&envelope.event, &crate::WsError::internal()))
                }
            }
        }
        WsReply::None => None,
        WsReply::Error(error) => Some(error_frame(&event, &error)),
    }
}

/// What a message past its deadline is answered: an error frame in the
/// class HTTP's request timeout answers (`UNAVAILABLE`), with `timeout` as its
/// reason, said once at `warn` for the operator.
fn timed_out(conn_id: ConnId, event: &str, timeout: Duration) -> Answered {
    tracing::warn!(
        target: crate::TARGET,
        conn_id,
        event,
        reason = TIMEOUT_REASON,
        timeout_secs = timeout.as_secs(),
        "websocket message not answered within its deadline; it is cut",
    );
    Answered::failed(WsReply::Error(crate::WsError::with_details(
        "the message was not answered in time",
        serde_json::json!({
            "code": nest_rs_core::problem::code::UNAVAILABLE.as_str(),
            "reason": TIMEOUT_REASON,
        }),
    )))
}

/// The `reason` a deadline's refusal carries, beside its `code`.
const TIMEOUT_REASON: &str = "timeout";

fn error_frame(event: &str, error: &crate::WsError) -> String {
    WsEnvelope::encode(event, error).unwrap_or_else(|_| {
        format!(
            r#"{{"event":"error","data":{{"error":"{}","errors":{{"code":"{}"}}}}}}"#,
            nest_rs_core::OPAQUE_CLIENT_MESSAGE,
            nest_rs_core::problem::code::INTERNAL,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;

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
        let wiring = wiring();
        let connection = wiring.connection.clone();
        let trace_id = connection.trace_id().to_hex();

        under_connection(
            &wiring,
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

    impl nest_rs_core::Layer for DenyAll {}

    #[async_trait]
    impl nest_rs_guards::Guard for DenyAll {
        async fn check(
            &self,
            _cx: &mut nest_rs_core::UnitContext<'_>,
        ) -> Result<(), nest_rs_guards::Denial> {
            Err(nest_rs_guards::Denial::forbidden(
                "author `banned` is not allowed to post",
            ))
        }
    }

    struct Moderated;

    static MODERATED: nest_rs_core::Handler =
        nest_rs_core::__private::declare_handler(nest_rs_core::__private::HandlerDeclaration {
            unit: crate::unit::MESSAGE,
            host: TypeId::of::<Moderated>(),
            host_name: "Moderated",
            method: "moderated",
            address: "moderated",
            posture: nest_rs_core::Posture::Gated,
            meta: nest_rs_core::__private::MetaLevels::default,
        });

    fn denying(
        container: &Container,
        handler: &nest_rs_core::Handler,
    ) -> nest_rs_core::anyhow::Result<Option<Arc<dyn nest_rs_core::__private::GuardStage>>> {
        crate::guard::message_guard_stage(
            container,
            handler,
            crate::guard::MessageGuards {
                method: vec![nest_rs_guards::guard::<DenyAll>()],
                force: Vec::new(),
                gate: None,
            },
        )
    }

    static MODERATED_SITE: nest_rs_core::__private::Site = nest_rs_core::__private::declare_site(
        &MODERATED,
        nest_rs_core::__private::SiteDeclaration {
            layers: nest_rs_core::__private::SiteLayers::default,
            global: nest_rs_core::__private::GlobalLayers::ALL,
            guard_stage: Some(denying),
            class_gate: None,
        },
    );

    struct Echo;

    #[async_trait]
    impl Gateway for Echo {
        async fn __nestrs_handle(
            &self,
            _client: &WsClient,
            _event: &str,
            data: serde_json::Value,
        ) -> Result<WsReply, HandlerFailed> {
            Ok(WsReply::Reply(data))
        }

        fn __nestrs_gate(&self, _event: &str) -> Option<(&'static Handler, ClassGate)> {
            None
        }
    }

    fn wiring() -> DispatchWiring {
        DispatchWiring {
            unit_of_work: None,
            scopes: RequestScopes::new(Container::default()),
            connection: nest_rs_core::Correlation::minted(None),
            message_timeout: None,
            connect_timeout: None,
            disconnect_timeout: None,
        }
    }

    /// The table of a gateway serving `moderated`, whose guard refuses every
    /// message, and `open`, which runs nothing around its handler.
    fn moderated() -> MessageTable {
        let container = Container::builder()
            .provide(DenyAll)
            .build()
            .expect("the container builds");
        let mut messages = MessageTable::new();
        messages.mount(&container, "moderated", &MODERATED_SITE, &[], &[]);
        messages
    }

    #[tokio::test]
    async fn a_denied_message_answers_the_frame_its_check_rendered_and_files_nothing_more() {
        let logs = nest_rs_testing::LogCapture::install();
        let frame = handle_text(
            &Echo,
            &moderated(),
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
            "the structured half the stage rendered travels whole: {frame}",
        );
        assert!(
            logs.events()
                .iter()
                .all(|event| event.target != crate::TARGET || event.level != "warn"),
            "the stage filed the denial where it held it; the gateway adds no second line: {:#?}",
            logs.events(),
        );
    }

    #[tokio::test]
    async fn an_event_no_handler_serves_is_answered_unknown_and_runs_no_handler() {
        let frame = handle_text(
            &Echo,
            &MessageTable::new(),
            &wiring(),
            &WsClient::for_test(),
            r#"{"event":"open","data":"hi"}"#,
        )
        .await
        .expect("an error frame");
        assert!(!frame.contains("hi"), "{frame}");
        assert!(frame.contains("unknown event"), "{frame}");
    }

    /// A handler whose future holds `N` bytes across an await.
    struct Holding<const N: usize>;

    #[async_trait]
    impl<const N: usize> Gateway for Holding<N> {
        async fn __nestrs_handle(
            &self,
            _client: &WsClient,
            _event: &str,
            _data: serde_json::Value,
        ) -> Result<WsReply, HandlerFailed> {
            let held = [0_u8; N];
            tokio::task::yield_now().await;
            std::hint::black_box(&held);
            Ok(WsReply::None)
        }

        fn __nestrs_gate(&self, _event: &str) -> Option<(&'static Handler, ClassGate)> {
            None
        }
    }

    static HELD: nest_rs_core::Handler =
        nest_rs_core::__private::declare_handler(nest_rs_core::__private::HandlerDeclaration {
            unit: crate::unit::MESSAGE,
            host: TypeId::of::<Moderated>(),
            host_name: "Holding",
            method: "held",
            address: "held",
            posture: nest_rs_core::Posture::Gated,
            meta: nest_rs_core::__private::MetaLevels::default,
        });

    static HELD_SITE: nest_rs_core::__private::Site = nest_rs_core::__private::declare_site(
        &HELD,
        nest_rs_core::__private::SiteDeclaration {
            layers: nest_rs_core::__private::SiteLayers::default,
            global: nest_rs_core::__private::GlobalLayers::ALL,
            guard_stage: None,
            class_gate: None,
        },
    );

    /// What a message's whole path — its unit, its pipeline, its frame — weighs
    /// around a handler holding `N` bytes.
    fn a_message_around_a_handler_holding<const N: usize>() -> usize {
        let mut messages = MessageTable::new();
        messages.mount(&Container::default(), "held", &HELD_SITE, &[], &[]);
        let (gateway, wiring, client) = (Holding::<N>, wiring(), WsClient::for_test());
        size_of_val(&handle_text(
            &gateway,
            &messages,
            &wiring,
            &client,
            r#"{"event":"held"}"#,
        ))
    }

    /// A debug build reserves every copy of a future a frame holds on the
    /// thread polling it: a message copying its handler's would multiply the
    /// stack a message needs by the stages it runs.
    #[test]
    fn a_message_holds_its_handlers_future_behind_one_box() {
        let light = a_message_around_a_handler_holding::<1024>();
        let heavy = a_message_around_a_handler_holding::<16384>();
        assert_eq!(
            light, heavy,
            "a message copies its handler's future: {light} bytes around a 1 KiB one, {heavy} \
             around a 16 KiB one",
        );
    }

    /// A writer that panics ends its socket: the read loop stops with it, the
    /// disconnect hook runs, the registry lets the connection go, and the peer
    /// reads the end of the stream with no Close frame, §7.4.1's 1006.
    #[tokio::test]
    async fn a_writer_that_panics_over_a_socket_ends_it_and_the_disconnect_hook_runs() {
        use tokio_tungstenite::tungstenite::protocol::Role;

        let logs = nest_rs_testing::LogCapture::install();
        let (server_end, client_end) = tokio::io::duplex(64 * 1024);
        let server_socket =
            tokio_tungstenite::WebSocketStream::from_raw_socket(server_end, Role::Server, None)
                .await;
        let mut client =
            tokio_tungstenite::WebSocketStream::from_raw_socket(client_end, Role::Client, None)
                .await;
        let server = Arc::new(WsServer::<crate::server::Global>::default());
        let serving = tokio::spawn(serve_connection(
            Arc::new(Echo),
            Arc::clone(&server),
            Arc::new(MessageTable::new()),
            wiring(),
            SocketLimits {
                max_lifetime: None,
                max_message_bytes: 1024,
            },
            server_socket.with(
                |_: Message| -> std::future::Ready<Result<Message, websocket::Error>> {
                    panic!("the writer exploded")
                },
            ),
            std::future::pending(),
        ));

        client
            .send(Message::text(r#"{"event":"open","data":"hi"}"#))
            .await
            .expect("the message reaches the server");
        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), serving)
            .await
            .expect("the connection ends with its writer");
        assert!(ended.is_ok(), "the connection task itself never panics");

        assert_eq!(server.connection_count(), 0, "the registry let it go");
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), client.next())
            .await
            .expect("the peer sees the end");
        assert!(
            !matches!(read, Some(Ok(Message::Close(_)))),
            "no Close frame reaches the peer once the writer went down: {read:?}",
        );
        let panicked = logs.expect_one(crate::TARGET, "writer task panicked");
        assert_eq!(panicked.level, "error");
        let said = panicked
            .field(nest_rs_core::panic::FIELD)
            .unwrap_or_default();
        assert!(said.contains("the writer exploded"), "{panicked:#?}");
        let left = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::DISCONNECT.name(),
        );
        assert_eq!(
            left.field("outcome").as_deref(),
            Some(operation_log::OK),
            "the disconnect hook ran: {left:#?}",
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
