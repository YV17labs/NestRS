use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::FutureExt as _;
use futures_util::future::Either;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use nest_rs_core::__private::{ClassGate, Pending, RequestScopes, install_pending};
use nest_rs_core::code;
use nest_rs_core::operation_log::{FiledOnce, Line, Unwinding};
use nest_rs_core::{
    AmbientValue, Container, Continuation, Correlation, Handler, RequestContinuation, operation_log,
};
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_guards::__private::GuardSpec;
use nest_rs_http::__private::websocket::{
    self, CloseCode, Due, KeepAlive, Message, Signal, WebSocketLimits, WebSocketUpgrade,
};
use nest_rs_http::{BoxEndpoint, DetachedWork, Endpoint, Request, Response};
use nest_rs_pipes::__private::PipeSpec;
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
    /// pipes and the call — what a message's unit runs
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
    /// that handler: `None` for a posture naming no subject, or an event none
    /// serves.
    #[doc(hidden)]
    fn __nestrs_gate(&self, event: &str) -> Option<(&'static Handler, ClassGate)>;

    /// Call the handler `event` names on `data`, as a test calls it on its
    /// own: the posture's class gate, its parameters, its payload's pipes and
    /// the call, a refusal or an error answered as its frame and filed at once.
    /// No guard and no layer runs here — a message arriving on a socket runs them
    /// around the handler, the class gate after its guards.
    async fn dispatch(&self, client: &WsClient, event: &str, data: serde_json::Value) -> WsReply {
        let (data, attached) = match self.__nestrs_gate(event) {
            Some((handler, gate)) => match crate::guard::gate_alone(client, handler, gate, data) {
                Ok(passed) => passed,
                Err(refused) => return WsReply::Error(refused),
            },
            None => (data, Pending::default()),
        };
        match install_pending(attached, self.__nestrs_handle(client, event, data)).await {
            Ok(reply) => reply,
            Err(failed) => failed.into_reply(event),
        }
    }

    /// Runs once when a socket connects, after the upgrade guards pass. One
    /// that panics, or outlasts its deadline, leaves the connection set up
    /// halfway, so the socket is not served: it is closed with RFC 6455
    /// §7.4.1's 1011 Internal Error. A hook states no posture naming what it
    /// reads, so the ability the upgrade attached stays off its reads, as off
    /// [`on_disconnect`](Self::on_disconnect)'s.
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

/// The label of the endpoint a gateway self-mounts its upgrade on, read back
/// where a gateway is told from the other self-mounts.
pub const GATEWAY_LABEL: &str = "ws";

/// The endpoint a gateway's upgrade mounts at, assembled from the gateway, its
/// messages, the container it mounts from, and the [`DetachedWork`] its
/// `HttpEndpointMeta` declares; what each connection needs from the container
/// is resolved here, once. `host` and `path`, the one a client connects to under
/// the global prefix, name the gateway as its `mounted` lines do, on every line
/// its sockets file; a unit deadline the shutdown window cannot hold is said
/// here at `warn`, naming it so. Called by `#[messages]`-generated mount code.
pub fn gateway_endpoint<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    messages: MessageTable,
    container: &Container,
    sockets: DetachedWork,
    host: &'static str,
    path: &str,
) -> BoxEndpoint {
    // Falls back to the bounded default, never to an unbounded lifetime or buffer.
    let config = container.get::<WsConfig>().unwrap_or_default();
    let window = container
        .get::<nest_rs_http::HttpConfig>()
        .unwrap_or_default()
        .shutdown_timeout;
    config.warn_when_units_outlast_the_window(window, path);
    BoxEndpoint::new(GatewayEndpoint {
        gateway,
        server,
        messages: Arc::new(messages),
        unit_of_work: container.get_dyn::<dyn UnitOfWork>(),
        config,
        scopes: RequestScopes::new(container.clone()),
        sockets,
        served: Arc::new(ServedGateway {
            host,
            path: Arc::from(path),
        }),
    })
}

/// The gateway as its `mounted` lines name it, which every line its sockets
/// file names too: its host, and the path a client connects to. Ambient for
/// each socket, so what a message's handler calls — its reply's serializer — names
/// it as well.
struct ServedGateway {
    host: &'static str,
    path: Arc<str>,
}

impl AmbientValue for ServedGateway {}

/// The path the gateway running this unit is served at, as its `mounted` lines
/// name it; `None` off a socket, for a message dispatched as a test calls it.
pub fn served_path() -> Option<Arc<str>> {
    nest_rs_core::ambient::<ServedGateway>().map(|served| Arc::clone(&served.path))
}

/// The wiring `#[messages]` attaches for the gateway at `path`: its
/// `upgrade` guards are phase-validated and resolvable, what its `messages`
/// declare holds against them — one no guard covers says `#[public]`, and one
/// saying it can be reached with no credential — and each composes over its
/// `host_pipes` — a layer no imported module provides, or two `#[meta]` values
/// of one type at one level, fail the boot naming the site.
pub fn wire_gateway(
    container: &Container,
    path: &str,
    upgrade: &[GuardSpec],
    host_pipes: &[PipeSpec],
    messages: &[DeclaredMessage],
) -> nest_rs_core::anyhow::Result<()> {
    let upgrade = crate::guard::validate_upgrade(container, path, upgrade)?;
    crate::message::compose_declared(container, host_pipes, messages)?;
    crate::guard::check_posture(container, path, &upgrade, messages)?;
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
    served: Arc<ServedGateway>,
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
            served: Arc::clone(&self.served),
        };
        let limits = SocketLimits {
            max_lifetime,
            max_message_bytes,
            ping_interval: self.config.ping_interval,
            ping_timeout: self.config.ping_timeout,
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
                let served = Arc::clone(&wiring.served);
                let serving = nest_rs_core::with_ambient(
                    served,
                    serve_connection(
                        gateway, server, messages, wiring, limits, socket, going_away,
                    ),
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
    /// The gateway the socket's lines name.
    served: Arc<ServedGateway>,
}

/// Per-socket limits resolved once at upgrade from [`WsConfig`].
#[derive(Clone, Copy)]
struct SocketLimits {
    /// Socket-lifetime ceiling; `None` ⇒ unlimited.
    max_lifetime: Option<Duration>,
    /// Per-message byte cap (also enforced at the protocol layer).
    max_message_bytes: usize,
    /// The peer's silence that earns it a Ping; `None` ⇒ no Ping.
    ping_interval: Option<Duration>,
    /// What a Ping waits for any frame back.
    ping_timeout: Duration,
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
    let served = &*wiring.served;

    // The `Sink` comes back when the outbox closes, so the Close frame is written
    // after every reply already queued. A frame that cannot be written ends the
    // socket: a peer that stops reading sends nothing either.
    let (writer_ends, mut writer_stopped) = tokio::sync::oneshot::channel::<()>();
    // The read loop owns the keepalive; the Ping it owes goes out here, since
    // this task owns the `Sink`, and ahead of the replies: an outbox that never
    // empties would hold it back until its peer was dropped as silent. Polled
    // beside every frame, so lock-free, and not at all with the Pings off.
    let ping = Arc::new(Signal::default());
    let pinged = Arc::clone(&ping);
    let pings = limits.ping_interval.is_some();
    let writer_served = Arc::clone(&wiring.served);
    let mut writer = Writer(tokio::spawn(nest_rs_core::panic::contain(async move {
        loop {
            let frame = tokio::select! {
                biased;
                () = pinged.taken(), if pings => Message::Ping(Default::default()),
                frame = rx.recv() => match frame {
                    Some(frame) => Message::Text(frame),
                    None => break,
                },
            };
            if let Err(error) = sink.send(frame).await {
                stopped_writing(&writer_served, conn_id, &error);
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
    let mut keep_alive = KeepAlive::new(limits.ping_interval, limits.ping_timeout);

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
                    host = served.host,
                    path = &*served.path,
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
                    host = served.host,
                    path = &*served.path,
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
                if message.is_ok() {
                    keep_alive.heard();
                }
                let reply = match message {
                    // The boundary case the protocol-layer cap lets through; framing is
                    // intact, so the socket survives and the refusal goes in band.
                    Ok(Message::Text(text)) if text.len() > limits.max_message_bytes => {
                        refuse_oversize(
                            &messages,
                            &wiring,
                            &client,
                            text.len(),
                            limits.max_message_bytes,
                        )
                        .await
                    }
                    Ok(Message::Text(text)) => {
                        handle_text(&*gateway, &messages, &wiring, &client, text.as_str()).await
                    }
                    // RFC 6455 §7.4.1's 1003 is the other conformant answer; framing is
                    // intact, so the refusal goes in band.
                    Ok(Message::Binary(data)) => {
                        refuse_binary(&messages, &wiring, &client, data.len()).await
                    }
                    Ok(Message::Close(_)) => break Closure::Echo,
                    // tungstenite queues the Pong before a Ping reaches this loop,
                    // and a raw frame is never read.
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => None,
                    Err(err) => break read_failed(served, conn_id, &err, limits.max_message_bytes),
                };
                // The pushes' outbox, so ordering with broadcasts the handler
                // triggered is preserved.
                if let Some(reply) = reply
                    && outbox.try_send(reply.into()).is_err()
                {
                    break stalled_outbox(served, conn_id);
                }
            }
            // Last, so a frame that arrived in the same instant answers the Ping.
            due = keep_alive.due() => match due {
                Due::Ping => ping.raise(),
                Due::Silent => break peer_silent(served, conn_id, limits.ping_timeout),
            },
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
        if let Some(sink) = writer_ended((&mut writer.0).await, served, conn_id) {
            close_socket(sink, closure, served, conn_id).await;
        }
    })
    .await;
    if closed.is_err() {
        tracing::debug!(
            target: crate::TARGET,
            host = served.host,
            path = &*served.path,
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
fn writer_ended<S>(
    joined: Result<Written<S>, tokio::task::JoinError>,
    served: &ServedGateway,
    conn_id: u64,
) -> Option<S> {
    match joined {
        Ok(Ok(sink)) => Some(sink),
        Ok(Err(payload)) => {
            nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload.as_ref(),
                "writer task panicked",
                host = served.host,
                path = &*served.path,
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
fn stopped_writing(served: &ServedGateway, conn_id: ConnId, error: &websocket::Error) {
    if matches!(error, websocket::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut) {
        tracing::warn!(
            target: crate::TARGET,
            host = served.host,
            path = &*served.path,
            conn_id,
            error = %nest_rs_core::error_message(error),
            "the peer took nothing within the send deadline; it is cut off",
        );
    } else {
        tracing::debug!(
            target: crate::TARGET,
            host = served.host,
            path = &*served.path,
            conn_id,
            error = %nest_rs_core::error_message(error),
            "closing socket: a frame could not be written",
        );
    }
}

/// Close a socket whose peer stopped draining its full outbox.
fn stalled_outbox(served: &ServedGateway, conn_id: u64) -> Closure {
    tracing::warn!(
        target: crate::TARGET,
        host = served.host,
        path = &*served.path,
        conn_id,
        close_code = u16::from(CloseCode::Policy),
        "closing socket: the peer stopped draining and the outbox is full",
    );
    Closure::Server(CloseCode::Policy, OUTBOX_STALLED)
}

/// Close a socket whose read failed, under the code [`websocket::read_failed`]
/// reads off the failure. A message past the cap is a client refused, as a
/// `413` is, and the one line its frame earns, since no unit reads it: at
/// `warn`, naming the cap, never the frame. Any other failure is most often the
/// connection gone.
fn read_failed(
    served: &ServedGateway,
    conn_id: ConnId,
    error: &websocket::Error,
    max_message_bytes: usize,
) -> Closure {
    let (code, reason) = websocket::read_failed(error);
    if code == CloseCode::Size {
        tracing::warn!(
            target: crate::TARGET,
            host = served.host,
            path = &*served.path,
            conn_id,
            error = %nest_rs_core::error_message(error),
            close_code = u16::from(code),
            max_message_bytes,
            "closing socket: a message is past its cap",
        );
    } else {
        tracing::debug!(
            target: crate::TARGET,
            host = served.host,
            path = &*served.path,
            conn_id,
            error = %nest_rs_core::error_message(error),
            close_code = u16::from(code),
            "websocket read error",
        );
    }
    Closure::Server(code, reason)
}

/// Drop a socket whose peer sent nothing back within the Ping's timeout.
fn peer_silent(served: &ServedGateway, conn_id: ConnId, ping_timeout: Duration) -> Closure {
    tracing::warn!(
        target: crate::TARGET,
        host = served.host,
        path = &*served.path,
        conn_id,
        ping_timeout_ms = ping_timeout.as_millis(),
        "closing socket: the peer sent nothing back within the ping timeout",
    );
    Closure::PeerSilent
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
    /// The peer sent nothing back within its Ping's timeout, so it is taken
    /// for gone and sent no Close, which §7.1.7 lets an endpoint omit when the
    /// peer is unlikely to read it: it reads 1006, if anything.
    PeerSilent,
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
async fn close_socket<S>(
    mut sink: WsSink<S>,
    closure: Closure,
    served: &ServedGateway,
    conn_id: ConnId,
) where
    S: Sink<Message, Error = websocket::Error>,
{
    if let Closure::WriterStopped | Closure::PeerSilent = closure {
        return;
    }
    if let Closure::Server(code, reason) = closure
        && let Err(err) = sink.send(websocket::close_frame(code, reason)).await
    {
        tracing::debug!(
            target: crate::TARGET,
            host = served.host,
            path = &*served.path,
            conn_id,
            error = %nest_rs_core::error_message(&err),
            "websocket close frame undelivered",
        );
        return;
    }
    if let Err(err) = SinkExt::close(&mut sink).await {
        tracing::debug!(
            target: crate::TARGET,
            host = served.host,
            path = &*served.path,
            conn_id,
            error = %nest_rs_core::error_message(&err),
            "websocket close handshake unfinished",
        );
    }
}

/// Refuse a message past the per-message cap, telling the client under the
/// class HTTP's `413` names and, at `debug`, the operator.
async fn refuse_oversize(
    messages: &MessageTable,
    wiring: &DispatchWiring,
    client: &WsClient,
    bytes: usize,
    max_message_bytes: usize,
) -> Option<String> {
    refuse_unread(
        messages,
        wiring,
        client,
        || {
            tracing::debug!(
                target: crate::TARGET,
                path = &*wiring.served.path,
                conn_id = client.id(),
                bytes,
                max_message_bytes,
                "websocket message refused: over the per-message cap",
            );
        },
        &crate::WsError::classed("message too large", code::CONTENT_TOO_LARGE),
    )
    .await
}

/// Refuse a Binary frame (RFC 6455 §5.6), which carries no JSON envelope,
/// telling the client under the class HTTP's `415` names and, at `debug`, the
/// operator.
async fn refuse_binary(
    messages: &MessageTable,
    wiring: &DispatchWiring,
    client: &WsClient,
    bytes: usize,
) -> Option<String> {
    refuse_unread(
        messages,
        wiring,
        client,
        || {
            tracing::debug!(
                target: crate::TARGET,
                path = &*wiring.served.path,
                conn_id = client.id(),
                bytes,
                "websocket message refused: binary frames carry no envelope",
            );
        },
        &crate::WsError::classed(
            "binary frames are not supported",
            code::UNSUPPORTED_MEDIA_TYPE,
        ),
    )
    .await
}

/// Refuse a frame no handler could read — no envelope, a binary frame, one
/// past the cap — as the message it is: its `ws.message` line and span, under
/// `_OTHER`, never what the client sent; the site of what no handler reads;
/// `report` saying why at `debug` inside it. The client reads `refusal` on the
/// `error` event, or what a layer answered in its place.
async fn refuse_unread(
    messages: &MessageTable,
    wiring: &DispatchWiring,
    client: &WsClient,
    report: impl FnOnce() + Send,
    refusal: &crate::WsError,
) -> Option<String> {
    let reply = in_message_unit(
        wiring,
        client.id(),
        operation_log::OTHER,
        None,
        |deadline| async move {
            report();
            crate::message::unserved(
                messages,
                client,
                serde_json::Value::Null,
                &wiring.served.path,
                refusal,
                deadline,
            )
            .await
        },
    )
    .await;
    reply_frame(reply, "error", operation_log::OTHER, &wiring.served.path)
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
    let path = &*wiring.served.path;
    let line = UnitLine::open(unit, path, conn_id, connection.clone(), span.clone());
    let timeout = match unit {
        Unit::Connect => wiring.connect_timeout,
        Unit::Disconnect => wiring.disconnect_timeout,
        Unit::Message { .. } => wiring.message_timeout,
    };
    let deadline = timeout.map(|timeout| (timeout, tokio::time::Instant::now() + timeout));
    let spec = unit_spec(unit, deadline.map(|(_, at)| at));
    // A hook states no posture naming what it reads, so the ability the upgrade
    // attached stays off its reads, as on any such unit.
    let body = nest_rs_core::with_ambient(
        Arc::new(nest_rs_core::__private::UnmaskedReply),
        nest_rs_core::panic::contain(hook),
    )
    .map(HookEnd::from);
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
            UnitLine::unwound(line, &*payload);
            false
        }
        HookEnd::TimedOut(timeout) => {
            line.file(operation_log::ERROR);
            tracing::warn!(
                target: crate::TARGET,
                path,
                conn_id,
                unit = unit.declared().name(),
                timeout_ms = timeout.as_millis(),
                "websocket connection hook not finished within its deadline; it is cut",
            );
            false
        }
        HookEnd::Unhonoured(why) => {
            line.file(operation_log::ERROR);
            tracing::error!(
                target: crate::TARGET,
                path,
                conn_id,
                unit = unit.declared().name(),
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

/// One unit's line: by the end the connection loop saw, or `cancelled` when the
/// transport stopped the socket with this unit still running.
///
/// One macro call per unit: `name:` must be a constant at each call site.
struct UnitLine<'a> {
    unit: Unit<'a>,
    /// The path its gateway is served at, every unit's coordinate.
    path: &'a str,
    conn_id: ConnId,
    /// Entered to file the line: a `Drop` runs while the unit's future is torn
    /// down, not reliably inside the scope that future installed.
    correlation: Correlation,
    /// The unit's span, which records the outcome the line files.
    span: tracing::Span,
    started: std::time::Instant,
}

impl<'a> UnitLine<'a> {
    /// Nothing but the unit shares the future its line drops with: an unwind
    /// dropping it is the unit's own.
    fn open(
        unit: Unit<'a>,
        path: &'a str,
        conn_id: ConnId,
        correlation: Correlation,
        span: tracing::Span,
    ) -> FiledOnce<Self> {
        FiledOnce::new(
            Self {
                unit,
                path,
                conn_id,
                correlation,
                span,
                started: std::time::Instant::now(),
            },
            Unwinding::Own,
        )
    }

    /// File what a message answered: `ok`, or `error`, which its span records
    /// unless its client's own frame earned it, as an HTTP `4xx` fails nothing on
    /// the server's span.
    fn answered(mut line: FiledOnce<Self>, answered: &Answered) {
        if answered.refused_its_client() {
            line.span = tracing::Span::none();
        }
        line.file(if answered.failed {
            operation_log::ERROR
        } else {
            operation_log::OK
        });
    }

    /// The unit unwound: file `panic`, and give the operator the panic's text,
    /// which the client is never told.
    fn unwound(line: FiledOnce<Self>, payload: &(dyn std::any::Any + Send)) {
        let (unit, path, conn_id) = (line.unit, line.path, line.conn_id);
        let correlation = line.correlation.clone();
        line.file(operation_log::PANIC);
        RequestContinuation::new(None, correlation).enter(|| match unit {
            Unit::Message { event } => nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "websocket handler panicked; its client is answered with an internal error",
                conn_id,
                path,
                event,
            ),
            Unit::Connect => nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "websocket connect hook panicked; the socket is closed unserved",
                conn_id,
                path,
                close_code = u16::from(CloseCode::Error),
            ),
            Unit::Disconnect => nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload,
                "websocket disconnect hook panicked; the close goes on",
                conn_id,
                path,
            ),
        });
    }
}

impl Line for UnitLine<'_> {
    fn file(&mut self, outcome: &'static str) {
        let (path, conn_id) = (self.path, self.conn_id);
        let span = &self.span;
        let started = self.started;
        RequestContinuation::new(None, self.correlation.clone()).enter(|| match self.unit {
            Unit::Message { event } => nest_rs_core::operation_line!(
                crate::unit::MESSAGE,
                span: span,
                outcome: outcome,
                started: started,
                path,
                event,
                conn_id,
            ),
            Unit::Connect => nest_rs_core::operation_line!(
                crate::unit::CONNECT,
                span: span,
                outcome: outcome,
                started: started,
                path,
                conn_id,
            ),
            Unit::Disconnect => nest_rs_core::operation_line!(
                crate::unit::DISCONNECT,
                span: span,
                outcome: outcome,
                started: started,
                path,
                conn_id,
            ),
        });
    }
}

/// Decode one text message and answer it as one unit of work: its site's
/// chains around its handler, inside the unit of work it runs, cut at its
/// deadline — the guards inside the unit, so they see the ambient values the
/// handler does, and a denied message opens no transaction. What no handler
/// serves runs the site of it instead, and, as a route's `404`, opens no unit
/// of work.
async fn handle_text<G: Gateway>(
    gateway: &G,
    messages: &MessageTable,
    wiring: &DispatchWiring,
    client: &WsClient,
    text: &str,
) -> Option<String> {
    let conn_id = client.id();
    let WsEnvelope { event, data } = match crate::envelope::decode_envelope(text) {
        Ok(envelope) => envelope,
        Err(refused) => {
            return refuse_unread(
                messages,
                wiring,
                client,
                || {
                    tracing::debug!(
                        target: crate::TARGET,
                        path = &*wiring.served.path,
                        conn_id,
                        error = %refused,
                        "websocket message refused: no envelope",
                    );
                },
                &refused,
            )
            .await;
        }
    };
    let served = messages.get(&event);
    // What the lines and the span name: the event a handler declares, never
    // one only the client named.
    let named = served.map_or(operation_log::OTHER, |(event, _)| event);
    let unit_of_work = served.and(wiring.unit_of_work.as_ref());
    let reply = in_message_unit(
        wiring,
        conn_id,
        named,
        unit_of_work,
        |deadline| async move {
            match served {
                Some((event, Some(message))) => {
                    crate::message::answer(
                        gateway,
                        message,
                        client,
                        event,
                        &wiring.served.path,
                        data,
                        deadline,
                    )
                    .await
                }
                Some((_, None)) => crate::message::refused(),
                None => {
                    let refusal = crate::WsError::unknown_event();
                    crate::message::unserved(
                        messages,
                        client,
                        data,
                        &wiring.served.path,
                        &refusal,
                        deadline,
                    )
                    .await
                }
            }
        },
    )
    .await;
    reply_frame(reply, &event, named, &wiring.served.path)
}

/// Run what `body` builds for the message's deadline as the message `named`
/// names: its span and line, a request scope of its own, a panic contained so
/// the socket goes on serving, inside `unit_of_work` when it opens one, the
/// body cut at that deadline.
async fn in_message_unit<B: Future<Output = Answered> + Send>(
    wiring: &DispatchWiring,
    conn_id: ConnId,
    named: &'static str,
    unit_of_work: Option<&Arc<dyn UnitOfWork>>,
    body: impl FnOnce(Option<tokio::time::Instant>) -> B,
) -> WsReply {
    let deadline = wiring
        .message_timeout
        .map(|timeout| (timeout, tokio::time::Instant::now() + timeout));
    let at = deadline.map(|(_, at)| at);
    let body = body(at);
    let path = &*wiring.served.path;
    // The body alone is cut: the unit settles outside it, so a commit never is.
    let cut = async move {
        match deadline {
            Some((timeout, at)) => tokio::time::timeout_at(at, body)
                .await
                .unwrap_or_else(|_| timed_out(conn_id, path, named, timeout)),
            None => body.await,
        }
    };
    let message = Unit::Message { event: named };
    let spec = unit_spec(message, at);
    let unit = run_unit(
        unit_of_work,
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
    let line = UnitLine::open(message, path, conn_id, correlation.clone(), span.clone());
    let ran = nest_rs_core::with_request_scope(
        Some(wiring.scopes.open()),
        correlation,
        nest_rs_core::panic::contain(unit),
    )
    .instrument(span)
    .await;
    match ran {
        Ok(answered) => {
            UnitLine::answered(line, &answered);
            answered.reply
        }
        Err(payload) => {
            UnitLine::unwound(line, &*payload);
            WsReply::Error(crate::WsError::internal())
        }
    }
}

/// The frame `reply` puts on the wire under `event`, none for silence; `named`
/// at `path` is what the operator's line calls the message.
fn reply_frame(reply: WsReply, event: &str, named: &str, path: &str) -> Option<String> {
    match reply {
        WsReply::Reply(data) => match WsEnvelope::encode(event, &data) {
            Ok(frame) => Some(frame),
            Err(err) => {
                tracing::warn!(
                    target: crate::TARGET,
                    path,
                    event = %named,
                    error = %nest_rs_core::error_message(&err),
                    "failed to serialize reply",
                );
                Some(error_frame(event, &crate::WsError::internal()))
            }
        },
        WsReply::None => None,
        WsReply::Error(error) => Some(error_frame(event, &error)),
    }
}

/// What a message past its deadline is answered: an error frame in the
/// class HTTP's request timeout answers (`UNAVAILABLE`), with `timeout` as its
/// reason, said once at `warn` for the operator, naming the message at `path`.
fn timed_out(conn_id: ConnId, path: &str, event: &str, timeout: Duration) -> Answered {
    tracing::warn!(
        target: crate::TARGET,
        conn_id,
        path,
        event,
        reason = TIMEOUT_REASON,
        timeout_ms = timeout.as_millis(),
        "websocket message not answered within its deadline; it is cut",
    );
    Answered::server_failed(WsReply::Error(
        crate::WsError::classed("the message was not answered in time", code::UNAVAILABLE)
            .with_reason(TIMEOUT_REASON),
    ))
}

/// The `reason` a deadline's refusal carries, beside its `code`.
const TIMEOUT_REASON: &str = "timeout";

fn error_frame(event: &str, error: &crate::WsError) -> String {
    WsEnvelope::encode(event, error).unwrap_or_else(|_| {
        serde_json::json!({
            "event": "error",
            "data": {
                "error": nest_rs_core::OPAQUE_CLIENT_MESSAGE,
                "code": code::INTERNAL.as_str(),
            },
        })
        .to_string()
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

        assert_eq!(writer_ended(joined, &wiring().served, 7), None);
        let line = logs.expect_one(crate::TARGET, "writer task panicked");
        assert_eq!(line.level, "error");
        assert_eq!(line.field("conn_id").as_deref(), Some("7"));
        assert_eq!(line.field("host").as_deref(), Some("Echo"));
        assert_eq!(line.field("path").as_deref(), Some("/echo"));
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
    async fn a_message_over_the_cap_is_refused_to_the_client_and_files_its_line() {
        let logs = nest_rs_testing::LogCapture::install();
        let wiring = wiring();
        let trace_id = wiring.connection.trace_id().to_string();
        let client = WsClient::for_test();

        let frame = refuse_oversize(&no_message(), &wiring, &client, 4096, 1024)
            .await
            .expect("a frame");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&frame).expect("a frame"),
            serde_json::json!({
                "event": "error",
                "data": { "error": "message too large", "code": "CONTENT_TOO_LARGE" },
            }),
            "the client is told why, and the class HTTP's 413 names",
        );
        let event =
            assert_refused_unread(&logs, "websocket message refused: over the per-message cap");
        assert_eq!(
            event.trace_id.as_deref(),
            Some(trace_id.as_str()),
            "the refusal joins the connection's conversation: {event:#?}",
        );
        assert_eq!(event.field("conn_id"), Some(client.id().to_string()));
        assert_eq!(event.field("bytes").as_deref(), Some("4096"));
        assert_eq!(event.field("max_message_bytes").as_deref(), Some("1024"));
    }

    #[derive(nest_rs_core::Layer)]
    struct DenyAll;

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
                upgrade: Vec::new(),
                method: vec![nest_rs_guards::__private::guard::<DenyAll>()],
                force: Vec::new(),
                gate: None,
                withheld: false,
                ambient: Vec::new(),
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
            served: Arc::new(ServedGateway {
                host: "Echo",
                path: Arc::from("/echo"),
            }),
        }
    }

    /// The table of a gateway serving no message.
    fn no_message() -> MessageTable {
        MessageTable::new(&Container::default())
    }

    /// A socket's two ends over an in-memory pipe holding `capacity` bytes, the
    /// server's first.
    async fn socket_pair(
        capacity: usize,
    ) -> (
        tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>,
        tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>,
    ) {
        use tokio_tungstenite::tungstenite::protocol::Role;

        let (server_end, client_end) = tokio::io::duplex(capacity);
        let server =
            tokio_tungstenite::WebSocketStream::from_raw_socket(server_end, Role::Server, None)
                .await;
        let client =
            tokio_tungstenite::WebSocketStream::from_raw_socket(client_end, Role::Client, None)
                .await;
        (server, client)
    }

    /// The table of a gateway serving `moderated`, whose guard refuses every
    /// message, and `open`, which runs nothing around its handler.
    fn moderated() -> MessageTable {
        let container = Container::builder()
            .provide(DenyAll)
            .build()
            .expect("the container builds");
        let mut messages = MessageTable::new(&container);
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
            frame["data"]["reason"], "forbidden",
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
            &no_message(),
            &wiring(),
            &WsClient::for_test(),
            r#"{"event":"open","data":"hi"}"#,
        )
        .await
        .expect("an error frame");
        assert!(!frame.contains("hi"), "{frame}");
        assert!(frame.contains("unknown event"), "{frame}");
    }

    /// A text message that is no envelope names the class HTTP's `400` names
    /// for a body that does not decode, and says where, never the value. It is
    /// a message all the same: its line files `error` under `_OTHER`, never
    /// what the client sent, and the operator reads where it failed at
    /// `debug`, with no `warn`. Only an object is an envelope: an array is
    /// never read by position, and the refusal names the shape, never a type.
    #[tokio::test]
    async fn a_message_that_is_no_envelope_names_the_invalid_argument_class_and_files_its_line() {
        const NOT_AN_OBJECT: &str = "expected an object with `event` and `data`";
        let refused =
            |kind: &str| format!("invalid envelope: invalid type: {kind}, {NOT_AN_OBJECT}");
        for (text, said) in [
            (
                r#"{"data":"sk_live_secret"}"#.to_owned(),
                "invalid envelope at `event`: missing".to_owned(),
            ),
            (
                r#"{"event":7,"data":"sk_live_secret"}"#.to_owned(),
                "invalid envelope at `event`: invalid type: an integer, expected a string"
                    .to_owned(),
            ),
            (
                r#"{"event":"open","data":1,"event":"sk_live_secret"}"#.to_owned(),
                "invalid envelope at `event`: duplicate".to_owned(),
            ),
            (r#""sk_live_secret""#.to_owned(), refused("a string")),
            (r#"["sk_live_secret"]"#.to_owned(), refused("a sequence")),
            (
                r#"["open", {"text":"sk_live_secret"}]"#.to_owned(),
                refused("a sequence"),
            ),
            ("7".to_owned(), refused("an integer")),
            ("null".to_owned(), refused("null")),
            (
                r#"{"event":"sk_live_secret""#.to_owned(),
                "invalid envelope: EOF while parsing an object at line 1 column 25".to_owned(),
            ),
        ] {
            let logs = nest_rs_testing::LogCapture::install();
            let frame = handle_text(
                &Echo,
                &no_message(),
                &wiring(),
                &WsClient::for_test(),
                &text,
            )
            .await
            .expect("an error frame");
            let frame: serde_json::Value = serde_json::from_str(&frame).expect("a JSON frame");
            assert_eq!(
                frame,
                serde_json::json!({
                    "event": "error",
                    "data": { "error": said, "code": "INVALID_ARGUMENT" },
                }),
                "{text}",
            );

            let report = assert_refused_unread(&logs, "websocket message refused: no envelope");
            assert_eq!(
                report.field("error").as_deref(),
                Some(said.as_str()),
                "{text}"
            );
        }
    }

    /// The `ws.message` line and span of a frame refused before any handler
    /// read it, filed once under `_OTHER`, the span recording no error of the
    /// server's; its report, `report`, at `debug`, joined to the line's
    /// correlation, and returned; no `warn`; no value the client sent.
    fn assert_refused_unread(
        logs: &nest_rs_testing::LogCapture,
        report: &str,
    ) -> nest_rs_testing::CapturedEvent {
        let line = logs.expect_one(operation_log::TARGET, crate::unit::MESSAGE.name());
        assert_eq!(line.field("outcome").as_deref(), Some(operation_log::ERROR));
        assert_eq!(line.field("event").as_deref(), Some(operation_log::OTHER));
        assert!(line.field("conn_id").is_some(), "{line:#?}");
        let span = logs.expect_span(crate::TARGET, crate::unit::MESSAGE.name());
        assert_eq!(
            span.field("ws.event").as_deref(),
            Some(operation_log::OTHER)
        );
        assert_eq!(span.field("error.type"), None, "{:?}", span.fields);
        assert_eq!(span.field("otel.status_code"), None, "{:?}", span.fields);
        let reported = logs.expect_one(crate::TARGET, report);
        assert_eq!(reported.level, "debug");
        assert_eq!(
            reported.field("path").as_deref(),
            Some("/echo"),
            "the report names the gateway as the message's line does: {reported:#?}",
        );
        assert!(line.span_id.is_some(), "the line names its unit: {line:#?}");
        assert_eq!(
            (&reported.trace_id, &reported.span_id),
            (&line.trace_id, &line.span_id),
            "the report joins the message's line",
        );
        assert!(
            logs.events_at(tracing::Level::WARN).is_empty(),
            "a client's error is no denial: {:#?}",
            logs.events(),
        );
        for event in logs.events() {
            assert!(
                event
                    .fields
                    .values()
                    .all(|value| !value.contains("sk_live")),
                "{event:#?}"
            );
        }
        reported
    }

    #[tokio::test]
    async fn a_binary_frame_is_refused_to_the_client_and_files_its_line() {
        let logs = nest_rs_testing::LogCapture::install();
        let frame = refuse_binary(&no_message(), &wiring(), &WsClient::for_test(), 3)
            .await
            .expect("a frame");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&frame).expect("a frame"),
            serde_json::json!({
                "event": "error",
                "data": {
                    "error": "binary frames are not supported",
                    "code": "UNSUPPORTED_MEDIA_TYPE",
                },
            }),
            "the client is told why, and the class HTTP's 415 names",
        );
        let event = assert_refused_unread(
            &logs,
            "websocket message refused: binary frames carry no envelope",
        );
        assert_eq!(event.field("bytes").as_deref(), Some("3"));
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
        let mut messages = no_message();
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
        let logs = nest_rs_testing::LogCapture::install();
        let (server_socket, mut client) = socket_pair(64 * 1024).await;
        let server = Arc::new(WsServer::<crate::server::Global>::default());
        let serving = tokio::spawn(serve_connection(
            Arc::new(Echo),
            Arc::clone(&server),
            Arc::new(no_message()),
            wiring(),
            SocketLimits {
                max_lifetime: None,
                max_message_bytes: 1024,
                ping_interval: None,
                ping_timeout: nest_rs_http::DEFAULT_PING_TIMEOUT,
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

    /// A peer answering every Ping keeps its socket, pinged once per interval of
    /// its silence; once it answers none, past the Ping's timeout the socket is
    /// dropped without a Close, the disconnect hook runs and the registry lets it
    /// go. Over an in-memory pipe, so paused time never outruns a frame in
    /// flight, as it does a kernel's.
    #[tokio::test(start_paused = true)]
    async fn a_peer_answering_its_pings_keeps_its_socket_and_one_answering_none_is_dropped() {
        use tokio::time::Instant;

        const INTERVAL: Duration = Duration::from_secs(20);
        const TIMEOUT: Duration = Duration::from_secs(5);
        const ROUNDS: u32 = 30;

        let logs = nest_rs_testing::LogCapture::install();
        let (server_socket, mut client) = socket_pair(64 * 1024).await;
        let server = Arc::new(WsServer::<crate::server::Global>::default());
        let start = Instant::now();
        let mut serving = tokio::spawn(serve_connection(
            Arc::new(Echo),
            Arc::clone(&server),
            Arc::new(no_message()),
            wiring(),
            SocketLimits {
                max_lifetime: None,
                max_message_bytes: 1024,
                ping_interval: Some(INTERVAL),
                ping_timeout: TIMEOUT,
            },
            server_socket,
            std::future::pending(),
        ));

        // Each read flushes the Pong tungstenite queued for the Ping before it.
        let mut pinged_at = Vec::new();
        let answering = tokio::time::sleep(INTERVAL * ROUNDS + INTERVAL / 2);
        tokio::pin!(answering);
        loop {
            tokio::select! {
                () = &mut answering => break,
                read = client.next() => match read {
                    Some(Ok(Message::Ping(_))) => pinged_at.push(Instant::now() - start),
                    other => panic!("only Pings reach a quiet peer: {other:?}"),
                },
            }
        }
        let expected: Vec<Duration> = (1..=ROUNDS).map(|round| INTERVAL * round).collect();
        assert_eq!(
            pinged_at, expected,
            "once per interval of the peer's silence"
        );
        assert_eq!(
            server.connection_count(),
            1,
            "an answering peer keeps its socket"
        );

        let last_ping = *pinged_at.last().expect("pinged");
        let dropped = tokio::time::timeout(INTERVAL * 2 + TIMEOUT, &mut serving)
            .await
            .expect("the socket is dropped once its peer answers no Ping");
        assert!(dropped.is_ok(), "the connection task itself never panics");
        assert_eq!(
            Instant::now() - start,
            last_ping + INTERVAL + TIMEOUT,
            "past the Ping the peer never answered, by its timeout",
        );
        assert_eq!(server.connection_count(), 0, "the registry let it go");
        let mut read = Vec::new();
        while let Some(frame) = client.next().await {
            let failed = frame.is_err();
            read.push(frame);
            if failed {
                break;
            }
        }
        assert!(
            matches!(read.as_slice(), [Ok(Message::Ping(_)), ..])
                && !read
                    .iter()
                    .any(|frame| matches!(frame, Ok(Message::Close(_)))),
            "its last Ping, then the end of the stream with no Close: {read:?}",
        );
        let line = logs.expect_one(
            crate::TARGET,
            "closing socket: the peer sent nothing back within the ping timeout",
        );
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("host").as_deref(), Some("Echo"));
        assert_eq!(line.field("path").as_deref(), Some("/echo"));
        let left = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::DISCONNECT.name(),
        );
        assert_eq!(left.field("outcome").as_deref(), Some(operation_log::OK));
    }

    /// A peer that only reads, from a socket whose outbox never empties, is
    /// pinged all the same and keeps it: the Ping goes out ahead of the frames
    /// queued before it, so pushes never hold it back until the peer, silent
    /// since it was never asked, is dropped.
    #[tokio::test(start_paused = true)]
    async fn a_peer_reading_a_socket_whose_outbox_never_empties_is_pinged_and_keeps_it() {
        const INTERVAL: Duration = Duration::from_secs(1);
        const TIMEOUT: Duration = Duration::from_secs(1);
        const READ_EVERY: Duration = Duration::from_millis(10);
        const BACKLOG: usize = 64;
        const HORIZON: Duration = Duration::from_secs(5);

        let logs = nest_rs_testing::LogCapture::install();
        let (server_socket, mut client) = socket_pair(4 * 1024).await;
        let server = Arc::new(WsServer::<crate::server::Global>::default());
        let _serving = tokio::spawn(serve_connection(
            Arc::new(Echo),
            Arc::clone(&server),
            Arc::new(no_message()),
            wiring(),
            SocketLimits {
                max_lifetime: None,
                max_message_bytes: 1024,
                ping_interval: Some(INTERVAL),
                ping_timeout: TIMEOUT,
            },
            server_socket,
            std::future::pending(),
        ));
        while server.connection_count() == 0 {
            tokio::task::yield_now().await;
        }
        // As many pushes as reads, after a backlog the pipe cannot hold.
        let pushing = Arc::clone(&server);
        let producer = tokio::spawn(async move {
            let tick = "x".repeat(256);
            for _ in 0..BACKLOG {
                pushing.broadcast("tick", &tick).expect("encodes");
            }
            loop {
                tokio::time::sleep(READ_EVERY).await;
                pushing.broadcast("tick", &tick).expect("encodes");
            }
        });

        let mut pings = 0_u32;
        let reading = async {
            loop {
                match client.next().await {
                    Some(Ok(Message::Ping(_))) => pings += 1,
                    Some(Ok(Message::Text(_))) => {}
                    other => return other,
                }
                tokio::time::sleep(READ_EVERY).await;
            }
        };
        let ended = tokio::time::timeout(HORIZON, reading).await;
        producer.abort();

        let silent = logs.find(
            crate::TARGET,
            "closing socket: the peer sent nothing back within the ping timeout",
        );
        assert!(
            ended.is_err() && server.connection_count() == 1 && silent.is_empty(),
            "a peer reading every frame keeps its socket: read {ended:?}, pinged {pings} times, \
             {} silent-peer lines",
            silent.len(),
        );
        assert!(
            pings >= 3,
            "pinged once per interval of its silence, the pushes notwithstanding: {pings} Pings",
        );
    }

    /// Pins the event, not the path: every `try_send` failure routes through
    /// `stalled_outbox`.
    #[test]
    fn a_peer_that_stops_draining_is_closed_with_a_reason() {
        let logs = nest_rs_testing::LogCapture::install();
        let closure = stalled_outbox(&wiring().served, 7);

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
