use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{FutureExt, SinkExt, StreamExt};
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

/// Per-connection message dispatcher a gateway implements. `#[messages]`
/// emits the impl: `dispatch` matches the event name, deserializes the
/// payload, calls the handler (passing `&WsClient` if it asks for one), and
/// wraps the return in [`WsReply`]. Never written by hand.
///
/// `on_connect` / `on_disconnect` are the `OnGatewayConnection` /
/// `OnGatewayDisconnect` analogs. The gateway is a singleton; hooks take
/// `&self` and the connecting socket's [`WsClient`].
#[async_trait]
pub trait Gateway: Send + Sync + 'static {
    /// Route one decoded message to its handler: match `event`, deserialize
    /// `data`, invoke the handler, and wrap the return in [`WsReply`]. Emitted
    /// by `#[messages]` — never hand-written.
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

/// A per-message data-pipe runner with the container already captured, so
/// `handle_text` (which has no container) can fold the global pipes over a
/// message's `data`. Built at mount by [`resolve_ws_data_pipe`].
pub type WsDataFold = dyn Fn(&str, &mut serde_json::Value) -> Result<(), PipeError> + Send + Sync;

/// Bridge slot for global pipes on a WS message's `data` — the per-message
/// analog of HTTP's `transform_body`. `nest-rs-guards`' `use_pipes_global`
/// provides a fn pointer that folds every registered global pipe's
/// [`GlobalPipe::transform_ws_data`](nest_rs_pipes::GlobalPipe) over the data.
/// Defined here (the gateway calls it), provided by guards (which owns the
/// `PipeSpecs` registry) — the same seeded-fn-pointer pattern as the GraphQL
/// `GraphqlVariablePipe`, since guards depends on this crate, not the reverse.
#[doc(hidden)] // Internal ABI — a seeded fn-pointer wired by the framework crates (lockstep).
pub struct WsDataPipe(pub fn(&Container, &str, &mut serde_json::Value) -> Result<(), PipeError>);

/// Resolve the [`WsDataPipe`] bridge at gateway mount into a runner with the
/// container captured. `None` when no global pipes are registered — the gateway
/// then skips the fold entirely.
pub fn resolve_ws_data_pipe(container: &Container) -> Option<Arc<WsDataFold>> {
    let bridge = container.get::<WsDataPipe>()?;
    let container = container.clone();
    Some(Arc::new(
        move |event: &str, data: &mut serde_json::Value| (bridge.0)(&container, event, data),
    ))
}

/// Assemble a [`GatewayEndpoint`] from a gateway and its resolved per-connection
/// wiring (registry, guard table, ambient context, global data-pipe fold), and
/// the [`DetachedWork`] its sockets are carried by — the one the gateway's
/// `HttpEndpointMeta` declares, so the transport tells every socket at the
/// shutdown signal and stops what is left at the close of its window. Called by
/// `#[gateway]`-generated mount code, not by hand.
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

/// The endpoint returned by [`gateway_endpoint`]. Generic over the gateway's
/// namespace `N` so it holds the gateway's own [`WsServer<N>`]; `N` never
/// escapes onto the handler surface.
pub struct GatewayEndpoint<G, N: 'static = crate::server::Global> {
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: Arc<EventLayerTable>,
    ctx: Option<Arc<dyn SocketContext>>,
    data_pipe: Option<Arc<WsDataFold>>,
    /// Every socket this gateway serves. poem stops tracking a connection at
    /// its upgrade, so without this the shutdown window neither waited for a
    /// socket nor closed it, and the transport returned past every one still
    /// open.
    sockets: DetachedWork,
}

impl<G: Gateway, N: 'static> Endpoint for GatewayEndpoint<G, N> {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        let (req, mut body) = req.split();
        let ws = WebSocket::from_request(&req, &mut body).await?;
        // Capture per-connection ambient state on the post-guard upgrade
        // request — connection-level guards have already attached the
        // principal/ability, and the request does not survive into the
        // connection task `on_upgrade` spawns.
        let ambient = self
            .ctx
            .as_ref()
            .map(|ctx| (ctx.clone(), ctx.capture(&req)));
        // Capture the singleton container from the request scope the HTTP
        // transport installs, so the connection loop can open a fresh
        // `RequestScope` per message for `Scoped<T>` resolution — the upgrade
        // request (and its scope) does not survive into the connection task.
        // `None` when the gateway is not nested under the HTTP request scope, in
        // which case per-message `Scoped<T>` resolves to `WsScopeError::NoScope`.
        let root_container =
            nest_rs_http::current_request_scope().map(|scope| scope.root().clone());
        // Resolve the WS config once per upgrade from the request scope the HTTP
        // transport installs. A missing scope or unregistered `WsConfig` falls
        // back to the (bounded) default — fail-secure, never a silently
        // unbounded socket lifetime nor an unbounded message buffer.
        let ws_config = nest_rs_http::current_request_scope()
            .and_then(|scope| scope.root().get::<WsConfig>())
            .unwrap_or_default();
        let max_lifetime = ws_config.max_connection;
        let max_message_bytes = ws_config.max_message_bytes;
        // Enforce the per-message cap at the WebSocket protocol layer so an
        // oversize frame is refused while reading — bounding buffering rather
        // than letting tungstenite buffer up to its 64 MiB default first (WS-I1).
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
            // The upgrade *is* an HTTP request, so the connection inherits that
            // request's id: "which request opened this socket" is answerable,
            // and every message this connection carries names it. Minted only
            // when the gateway is mounted outside the HTTP edge, which is a
            // wiring shape rather than a normal deployment.
            connection: nest_rs_core::Correlation::inherited(),
        };
        let limits = SocketLimits {
            max_lifetime,
            max_message_bytes,
        };
        let sockets = self.sockets.clone();
        Ok(ws
            .on_upgrade(move |socket| {
                // The socket inherits the upgrade's identity for its whole life
                // (see `under_connection`), and installing it once here is what
                // makes every line the connection loop emits carry the trace —
                // and `current_trace_id()` answer anywhere on this task. A
                // per-message install, which carries the *message's* own
                // correlation, still wins inside it.
                let connection = wiring.connection.clone();
                let going_away = sockets.going_away();
                async move {
                    // Carried by the transport: told at the signal, given the
                    // window to finish what it is answering, and dropped where it
                    // waits if it has not by the window's close.
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
/// every message: the ambient (executor + ability) seam, the global data-pipe
/// fold, and the singleton container each message's [`RequestScope`] is built
/// over. Bundled so the connection loop and `handle_text` stay under the
/// argument-count lint.
struct DispatchWiring {
    ambient: Option<(Arc<dyn SocketContext>, Captured)>,
    data_pipe: Option<Arc<WsDataFold>>,
    root_container: Option<Container>,
    /// The upgrade that opened this socket. Every message names its id, so a
    /// connection's whole conversation is one query — while each message keeps an
    /// id of its own, because a message is the unit of work, not the socket.
    ///
    /// It carries the **actor** too: the guards authenticated once, at the
    /// upgrade, and nothing re-authenticates per message — so without inheriting
    /// it here every message would be attributed to nobody.
    connection: nest_rs_core::Correlation,
}

/// Per-socket limits resolved once at upgrade from [`WsConfig`], threaded into
/// the connection loop together.
#[derive(Clone, Copy)]
struct SocketLimits {
    /// Socket-lifetime ceiling; `None` ⇒ unlimited.
    max_lifetime: Option<Duration>,
    /// Per-message byte cap (also enforced at the protocol layer).
    max_message_bytes: usize,
}

/// RAII cleanup for a connection's [`WsServer`] registry entry. Its `Drop`
/// removes the connection, so the entry — and the outbox `Sender` it holds —
/// cannot outlive the connection task even when gateway user code panics and
/// unwinds past the normal disconnect path (which would otherwise leak a dead
/// `Conn` holding a dead `Sender` in the registry map forever).
struct RegistryGuard<N: 'static> {
    server: Arc<WsServer<N>>,
    conn_id: ConnId,
}

impl<N: 'static> Drop for RegistryGuard<N> {
    fn drop(&mut self) {
        self.server.disconnect(self.conn_id);
    }
}

/// Drive one connection. The socket is split so server→client pushes all
/// funnel through one outbox drained by a writer task — decoupling the
/// read/dispatch loop from the single `Sink` and letting [`WsServer`] reach a
/// client it is not currently reading from.
///
/// `going_away` is the transport's shutdown signal: once it resolves the socket
/// reads nothing more and closes with 1001, after the message it is answering —
/// a message is read and answered whole inside the loop, so the signal is only
/// ever seen between two.
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

    // The task hands the `Sink` back when the outbox closes: the connection's
    // last act is a Close frame, and it has to be written *after* every reply
    // already queued — which is exactly what "the writer has finished" means.
    let mut writer = Writer(tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink.send(Message::Text(frame.to_string())).await.is_err() {
                break;
            }
        }
        sink
    }));

    let conn_id = server.connect(outbox.clone());
    // RAII cleanup: remove this connection's registry entry (which holds its
    // outbox `Sender`) on *every* exit path — including an unwind from
    // panicking gateway user code, which would otherwise leak a dead `Conn`.
    let registry_guard = RegistryGuard {
        server: Arc::clone(&server),
        conn_id,
    };
    // Type-erase the registry so `N` never surfaces on `WsClient`.
    let registry: Arc<dyn Registry> = server.clone();
    let client = WsClient::new(conn_id, registry);

    // `on_connect` and `on_disconnect` are units of work like any message —
    // developer code that logs, writes and emits — so each gets a span and the
    // ambient id, and both are the *connection's*: the upgrade is what accepted
    // this work, and nothing between opening and closing re-authenticates. Two
    // short spans rather than one wrapping the loop, because the message is the
    // unit here and a parent that stays open for the socket's life would file
    // every message under a span that never closes.
    let set_up = under_connection(
        &wiring.connection,
        Unit::Connect,
        conn_id,
        nest_rs_core::operation_span!(
            target: crate::TARGET,
            kind: nest_rs_core::operation_log::kind::SERVER,
            crate::unit::CONNECT,
            &wiring.connection,
            ws.connection_id = conn_id,
        ),
        gateway.on_connect(&client),
    )
    .await;

    // Optional socket-lifetime ceiling. When it elapses the server closes the
    // socket through the same path as a client `Close`, so a principal captured
    // once at the upgrade cannot outlive the ceiling (bounding the stale-privilege
    // window after token expiry/logout/revocation). `None` ⇒ unlimited, modeled
    // as an inert `select!` arm so an unbounded socket runs exactly as before.
    let mut lifetime = limits
        .max_lifetime
        .map(|ttl| Box::pin(tokio::time::sleep(ttl)));

    let mut going_away = std::pin::pin!(going_away);
    let closure = loop {
        if !set_up {
            break Closure::Server(CloseCode::Error, CONNECT_FAILED);
        }
        tokio::select! {
            // The server ends what it is shutting down before it reads another
            // message — and before the ceiling, which is the same close for
            // another reason.
            biased;
            () = &mut going_away => {
                // `debug`, unlike its siblings: the deployment asked for this,
                // and said so once ("shutdown signal received"); a line per
                // socket at `info` is the fleet's size in noise. The socket's
                // `ws.disconnect` line still files.
                tracing::debug!(
                    target: crate::TARGET,
                    conn_id,
                    close_code = u16::from(CloseCode::Away),
                    "closing socket: the server is going away",
                );
                break Closure::Server(CloseCode::Away, GOING_AWAY);
            }
            // Deadline arm: armed only when a ceiling is configured — otherwise
            // a `pending()` future that never resolves, leaving the read loop
            // untouched. The timer's deadline is absolute (set at connect), so
            // losing the `select!` race does not reset it.
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
                        // The boundary case the protocol-layer cap lets through
                        // (`WebSocketConfig` refuses *past* the limit while
                        // reading, which is what bounds buffering — WS-I1). The
                        // whole text is in hand, so framing is intact and the
                        // socket survives: the client is answered in band, in
                        // the envelope grammar it already parses. Its twin —
                        // the same message class refused by the protocol layer —
                        // arrives as a read error instead, with the framing gone
                        // mid-message, and that one can only end the socket.
                        // One class, two layers, and the answer follows what the
                        // *connection* can still do rather than what the message
                        // was.
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
                            // Replies ride the same outbox as pushes so ordering
                            // with broadcasts the handler triggered is preserved.
                            // A full outbox means the peer is not draining —
                            // disconnect it rather than buffer without bound.
                            if outbox.try_send(reply.into()).is_err() {
                                break stalled_outbox(conn_id);
                            }
                        }
                    }
                    // RFC 6455 §5.6 makes Binary a first-class data frame; this
                    // gateway's contract is a JSON text envelope, so it is
                    // refused — and *said*, at both ends. Dropping it was the
                    // one arm that answered nothing and logged nothing, which is
                    // indistinguishable from a handler that hung. Framing is
                    // intact, so the socket survives and the refusal travels in
                    // band; §7.4.1's 1003 (a text-only endpoint **MAY** close on
                    // a binary message) is the other conformant answer, and the
                    // in-band one is taken for the same reason the oversize
                    // boundary above takes it.
                    Ok(Message::Binary(data)) => {
                        let frame = refuse_binary(conn_id, data.len());
                        if outbox.try_send(frame.into()).is_err() {
                            break stalled_outbox(conn_id);
                        }
                    }
                    Ok(Message::Close(_)) => break Closure::Echo,
                    // Answered by the protocol layer itself — tungstenite queues
                    // a Pong before a Ping ever reaches this loop — so there is
                    // nothing here to do and nothing being dropped.
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

    // Fire `on_disconnect` while still registered, then drop the guard to
    // remove the entry. Dropping it *before* awaiting the writer releases the
    // registry's outbox `Sender` clone so the writer task observes the channel
    // close; on an unwind the guard's `Drop` does the same cleanup.
    under_connection(
        &wiring.connection,
        Unit::Disconnect,
        conn_id,
        nest_rs_core::operation_span!(
            target: crate::TARGET,
            kind: nest_rs_core::operation_log::kind::SERVER,
            crate::unit::DISCONNECT,
            &wiring.connection,
            ws.connection_id = conn_id,
        ),
        gateway.on_disconnect(&client),
    )
    .await;
    drop(registry_guard);
    drop(outbox);
    // Bounded: a peer that stopped reading parks the writer's sends and the
    // Close frame after them, and would hold the socket past the ceiling or the
    // signal that ended it. Past the grace the writer is aborted — `Writer`'s
    // `Drop` — and the socket goes with it.
    let closed = tokio::time::timeout(DetachedWork::CLOSE_GRACE, async {
        match (&mut writer.0).await {
            // Every queued reply is on the wire and the `Sink` is back, so the
            // Close frame lands last — the ordering §5.5.1 describes.
            Ok(sink) => close_socket(sink, closure, conn_id).await,
            // A `JoinError` here means the writer panicked: it is aborted only
            // after this wait. Surfaced rather than swallowed.
            Err(err) => {
                if err.is_panic() {
                    tracing::warn!(
                        target: crate::TARGET,
                        conn_id,
                        error = %nest_rs_core::error_message(&err),
                        "writer task failed",
                    );
                }
                // The `Sink` went down with the task, so there is nothing left
                // to close through and the peer reads 1006 — which is what
                // §7.4.1 defines a crashed endpoint to be.
            }
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
/// dropped before it is joined.
///
/// It owns the socket's sink, so a writer left running by a connection the
/// transport stopped — or by one whose peer stopped reading — kept the socket
/// open after everything that served it had gone.
struct Writer(tokio::task::JoinHandle<WsSink>);

impl Drop for Writer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The write half of a connection's socket.
type WsSink = futures_util::stream::SplitSink<poem::web::websocket::WebSocketStream, Message>;

/// The one server-initiated close that used to say nothing.
///
/// Its two siblings — the lifetime ceiling and the read error — each log with a
/// `close_code`, and [`Closure`]'s own doc gives the reason: without it "a
/// deliberate close indistinguishable from a broken pipe". A peer that stopped
/// draining is exactly the case an operator needs told apart from a network
/// fault, since it is the client's fault and it repeats.
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
/// RFC 6455 §7.4.1 status code the peer is told it in.
///
/// Every server-initiated termination used to drop the `Sink`, so the peer read
/// **1006 Abnormal Closure**, which §7.4.1 reserves for a connection "closed
/// without sending or receiving a Close frame" — a network fault. That makes a
/// deliberate close indistinguishable from a broken pipe, so a client retries
/// identically against both and never learns what it has to do differently. For
/// the socket-lifetime ceiling that difference is the whole point of the
/// ceiling: what it asks for is a fresh upgrade, and with it a fresh authn/authz
/// check (see [`WsConfig`](crate::WsConfig)).
enum Closure {
    /// The peer sent a Close, and §5.5.1 obliges the endpoint that *receives*
    /// one to send one back. The protocol layer has already queued that echo —
    /// tungstenite buffers the reply when it decodes the frame — so what this
    /// arm owes is the flush that puts it on the wire, which nothing did while
    /// the writer task simply dropped the `Sink`. Writing a second Close here
    /// would be refused rather than merged (`SendAfterClosing`).
    Echo,
    /// The stream ended without a Close frame: the peer is already gone, so
    /// there is nobody to tell and 1006 is the honest answer — §7.4.1 defines
    /// that case as exactly this one.
    PeerGone,
    /// The server ended it, under the code §7.4.1 defines for the cause.
    Server(CloseCode, &'static str),
}

/// §7.4.1 **1001 Going Away**, and not 1008 Policy Violation, which is the other
/// candidate. 1008 is defined for an endpoint terminating "because it **has
/// received a message** that violates its policy" — nothing the peer sent
/// reaches this ceiling, a clock does, so 1008 would name a cause that did not
/// happen. 1001 is the RFC's code for an endpoint that is deliberately ending a
/// connection it will no longer serve, and it is the one a client answers by
/// reconnecting — which is precisely the remedy: re-upgrade, and be
/// re-authenticated on the way in.
const LIFETIME_REACHED: &str = "connection lifetime reached, re-upgrade to continue";

/// §7.4.1 **1001 Going Away**, the code it gives for "a server going down" —
/// which is what the shutdown signal is. It is the one a client answers by
/// reconnecting, and a reconnect lands on a replica still in the load balancer.
const GOING_AWAY: &str = "the server is going away, reconnect to continue";

/// §7.4.1 **1011 Internal Error** — "an unexpected condition that prevented it
/// from fulfilling the request". The connect hook unwound, so the connection is
/// set up halfway and is not served; the client may simply try again.
const CONNECT_FAILED: &str = "the connection could not be set up";

/// §7.4.1 **1008 Policy Violation** — its "generic status code … when there is
/// no other more suitable" clause. A peer that will not drain a bounded outbox
/// is shed rather than buffered without bound, which is a policy of this
/// server's, not a fault of the connection's.
const OUTBOX_STALLED: &str = "outbox full, the client is not draining its messages";

/// §7.4.1 **1011 Internal Error** — "an unexpected condition that prevented it
/// from fulfilling the request". The read failed and poem hands the cause on as
/// an opaque [`std::io::Error`] (it stringifies every tungstenite variant that
/// is not itself I/O), so the protocol-layer message cap — which surfaces
/// *here*, as a read error — cannot be told apart from a framing fault. 1009
/// would be the precise code for the first and a false statement about the
/// second, and a code the peer cannot check is worse than the generic one.
const READ_FAILED: &str = "the connection could not be read";

/// Put the Close frame on the wire, then flush.
///
/// Both halves matter and neither substitutes for the other: the frame is what
/// carries the §7.4.1 code, and the flush is what reaches the peer — including
/// for [`Closure::Echo`], where the frame is the one the protocol layer queued
/// on our behalf and nothing had ever driven out.
///
/// Best-effort by construction: a peer that has already gone cannot be told
/// anything, so a failure here is `debug`, the level its siblings on the
/// client-shaped paths use.
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

/// Refuse a message past the per-message cap, and say so.
///
/// A refusal the client is told about and the operator is not is invisible
/// exactly where it matters: a deployment whose cap is set too low looks, from
/// the outside, like clients that quietly stopped sending. `debug` rather than
/// `warn`, matching the read-error path beside it — a message over a configured
/// size is a client-shaped error, not a security denial.
///
/// The frame is built here too, so the sentence the client reads and the event
/// the operator greps cannot come to describe different refusals.
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

/// Refuse a Binary frame, and say so.
///
/// RFC 6455 §5.6 makes Binary a first-class data frame, so a client is entitled
/// to send one; this gateway's contract is a JSON text envelope, so it cannot
/// route it. Refusing is right — refusing *silently* is what this arm did, and
/// from the client's side a dropped frame and a handler that never answered are
/// the same observation. `debug` rather than `warn`, for `refuse_oversize`'s
/// reason: a frame in the wrong format is a client-shaped error, not a security
/// denial.
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

/// Run one connection-lifecycle hook — `on_connect` / `on_disconnect` — under
/// the connection's identity, and say whether it ran to its end: `false` when
/// it unwound.
///
/// A hook is developer code that logs, writes and emits, so it is a unit of work
/// like any message; what differs is only whose id it takes. Both take the
/// *connection's*: the upgrade is what accepted this work, and nothing between
/// opening and closing re-authenticates anybody.
///
/// Both halves are needed and neither substitutes for the other — the span is
/// what puts `trace_id` on the events the hook emits, the ambient context is
/// what makes `current_trace_id()` answer inside it. The span is passed in
/// because `tracing` fixes a span's name at the macro, so the two call sites
/// name themselves (`ws.connect` / `ws.disconnect`) and share everything else.
///
/// A hook that panics is contained here and files `panic`: it unwound the
/// connection task, which took the socket down with no Close frame — 1006 to a
/// client that could not tell it from a network fault — and filed no line.
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
        AssertUnwindSafe(hook).catch_unwind(),
    )
    .instrument(span)
    .await;
    match ran {
        Ok(()) => {
            // `ok` is all a hook can report: it returns `()`, so there is no
            // failure signal for this line but an unwind.
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

/// One unit's line, held while the unit runs and filed exactly once — by the
/// end the connection loop saw, or by `Drop` when the unit is dropped first,
/// as [`CANCELLED`](operation_log::CANCELLED): the transport stopped the socket
/// at the close of its window, with this unit still running.
///
/// One `info!` per unit, so each carries its own constant as `name:` as well as
/// `message` — the two slots every line of the family fills, which a shared
/// call site taking the name as an argument could not.
struct UnitLine<'a> {
    unit: Unit<'a>,
    conn_id: ConnId,
    /// Entered to file the line, rather than read from the ambient context: a
    /// `Drop` runs while the unit's future is torn down, which is not reliably
    /// inside the scope that future installed.
    correlation: Correlation,
    /// The unit's span, which records the outcome the line files, in the same
    /// word.
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
            Unit::Message { event } => tracing::error!(
                target: crate::TARGET,
                conn_id,
                event,
                panic = nest_rs_core::panic_message(payload),
                "websocket handler panicked; its client is answered with an internal error",
            ),
            Unit::Connect => tracing::error!(
                target: crate::TARGET,
                conn_id,
                panic = nest_rs_core::panic_message(payload),
                close_code = u16::from(CloseCode::Error),
                "websocket connect hook panicked; the socket is closed unserved",
            ),
            Unit::Disconnect => tracing::error!(
                target: crate::TARGET,
                conn_id,
                panic = nest_rs_core::panic_message(payload),
                "websocket disconnect hook panicked; the close goes on",
            ),
        });
    }

    fn emit(&mut self, outcome: &'static str) {
        self.filed = true;
        operation_log::record_outcome(&self.span, outcome);
        let conn_id = self.conn_id;
        let duration_ms = operation_log::duration_ms(self.started);
        RequestContinuation::new(None, self.correlation.clone()).enter(|| match self.unit {
            // One line per message, in the message's own ids rather than the
            // socket's. A socket can serve thousands of messages under one
            // upgrade, so the `101`'s access line names the connection and says
            // nothing about the work — this is where that is said.
            Unit::Message { event } => tracing::info!(
                name: crate::unit::MESSAGE,
                target: nest_rs_core::operation_log::TARGET,
                message = crate::unit::MESSAGE,
                event,
                conn_id,
                outcome,
                duration_ms,
            ),
            // A hook is developer code that logs and writes like any handler,
            // so the socket opening and closing are units of work and owe the
            // family's line the same way a message does.
            Unit::Connect => tracing::info!(
                name: crate::unit::CONNECT,
                target: nest_rs_core::operation_log::TARGET,
                message = crate::unit::CONNECT,
                conn_id,
                outcome,
                duration_ms,
            ),
            Unit::Disconnect => tracing::info!(
                name: crate::unit::DISCONNECT,
                target: nest_rs_core::operation_log::TARGET,
                message = crate::unit::DISCONNECT,
                conn_id,
                outcome,
                duration_ms,
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

/// Per-message guards run **inside** a present [`SocketContext::around`], so
/// they see the same ambient task-locals the handler does — without that, a
/// per-message `Guard` reading `current_ability()` would see `None` and fail
/// closed on a mis-wired gateway. The no-context path runs guards then the
/// handler bare.
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
    // Bundle guard + dispatch so `around` wraps both — a guard reading
    // `current_ability()` / `current_executor()` sees the captured state.
    let event_ref = event.clone();
    let conn_id = client.id();
    let inner: BoxFuture<'_, WsReply> = Box::pin(async move {
        if let Err(reason) = guards.check(client, &event_ref, &data).await {
            // A per-message guard denial is a security event — it must be
            // greppable at `warn`+ like every other transport's denial, not
            // silently folded into the error reply (WS-I2). On `nest_rs::ws`,
            // not `nest_rs::layers`: the concern is a websocket message, and an
            // operator tailing this transport for denials filters by transport.
            // (`nest_rs::layers` stays the target for events *about* the layer
            // system itself, like a guard declared at two scopes.)
            tracing::warn!(
                target: crate::TARGET,
                conn_id,
                event = %event_ref,
                reason = %reason,
                "websocket message denied by a guard",
            );
            return WsReply::Error(crate::WsError::new(reason));
        }
        // Global data pipes run after guards (which see the raw value), before
        // dispatch — the per-message analog of HTTP running pipes after guards.
        if let Some(pipe) = data_pipe
            && let Err(err) = pipe(&event_ref, &mut data)
        {
            return WsReply::pipe_error(&event_ref, "data", err);
        }
        gateway.dispatch(client, &event_ref, data).await
    });
    // The ambient (executor + ability) seam wraps dispatch; the per-message
    // request scope wraps that, so a handler resolves `Scoped<T>` and reads the
    // captured task-locals together. A fresh `RequestScope` per message means an
    // `#[injectable(scope = request)]` provider is rebuilt for each message.
    let dispatch: BoxFuture<'_, WsReply> = match wiring.ambient.as_ref() {
        Some((ctx, captured)) => ctx.around(captured, inner),
        None => inner,
    };
    // A message is its own unit of work — its own id, its own span — carrying
    // the connection's id as a field so a socket's whole conversation still
    // groups. The span is what puts `trace_id` on every event a handler emits
    // and what declares the `actor_id` field the ability guard records into;
    // without it a WS handler's events were attributable to nothing at all.
    // A message is its own unit of work inside the socket's trace: same trace,
    // a fresh span, and the connection's span as its parent. The actor rides
    // along because nothing per message re-authenticates.
    let correlation = wiring.connection.child();
    let span = nest_rs_core::operation_span!(
        target: crate::TARGET,
        kind: nest_rs_core::operation_log::kind::SERVER,
        crate::unit::MESSAGE,
        &correlation,
        ws.event = %event,
        ws.connection_id = conn_id,
    );
    // The correlation is installed either way, and the kernel's installer is what
    // makes that free: a gateway with no container to open a scope over still
    // accepted a unit of work, and dropping the id there would make
    // `current_trace_id()` answer differently depending on how the app happened
    // to be wired. Only `Scoped<T>` goes without.
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
    // A handler that panics is contained at the message: it took the connection
    // task down with it, so the socket died with no Close frame and no line.
    // The client is answered as for any failed message, in a sentence that
    // says nothing of what unwound, and the socket goes on serving — the panic
    // is this message's, and nothing on the connection is left half done.
    let ran = nest_rs_core::with_request_scope(
        scope,
        correlation,
        AssertUnwindSafe(dispatch).catch_unwind(),
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
                    // A reply that cannot be re-serialized would otherwise vanish
                    // silently; log it and degrade to an error frame, mirroring
                    // `error_frame`'s own fallback rather than dropping the reply.
                    tracing::warn!(
                        target: crate::TARGET,
                        event = %envelope.event,
                        error = %nest_rs_core::error_message(&err),
                        "failed to serialize reply",
                    );
                    Some(error_frame(
                        &envelope.event,
                        &crate::WsError::new("internal error"),
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

    /// A connection hook is developer code, and until it ran under the
    /// connection's identity its events were attributable to nothing: the
    /// upgrade's request boundary is gone by the time `on_upgrade`'s task runs,
    /// so `on_connect` logged with no `trace_id` on the span and
    /// `current_trace_id()` answered `None` inside it.
    ///
    /// Both halves are asserted because neither implies the other — a span
    /// without the ambient context leaves the developer's own reads empty, and
    /// the ambient context without a span leaves the framework's events bare.
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
                target: crate::TARGET,
                kind: nest_rs_core::operation_log::kind::SERVER,
                crate::unit::CONNECT,
                &connection,
                ws.connection_id = 7u64,
            ),
            async {
                tracing::info!(
                    target: crate::TARGET,
                    // What the hook's own code can reach — the task-local half.
                    ambient = nest_rs_core::current_trace_id().map(|id| id.to_hex()),
                    // And the span it is emitted under — the half that carries
                    // `trace_id` onto events the hook never thought about.
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
            Some(crate::unit::CONNECT),
            "the hook's events are rooted at the connection span: {:?}",
            event.fields,
        );

        // And the socket opening is itself a unit of work, so it files the
        // family's line — a hook is developer code that logs and writes like any
        // handler, and a connection nobody can see opening is a connection
        // nobody can account for.
        let opened = logs.expect_one(nest_rs_core::operation_log::TARGET, crate::unit::CONNECT);
        assert_eq!(opened.message, crate::unit::CONNECT);
        assert_eq!(opened.field("conn_id").as_deref(), Some("7"));
        assert!(opened.field("duration_ms").is_some());
    }

    /// The client is told; the operator has to be told too. A cap set too low
    /// looks from the outside exactly like clients that stopped sending, and
    /// the only place that difference is visible is here.
    ///
    /// The refusal names no `trace_id` of its own: it runs under the connection
    /// scope the upgrade installs, which is where every line's correlation comes
    /// from. Asserted here as the ambient answer rather than as an event field —
    /// an event field would be the duplicate, and it is what the line used to
    /// carry twice.
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
        ) -> Result<(), String> {
            Err("author `banned` is not allowed to post".into())
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

    /// A per-message guard denial is a security event, so it has to be
    /// greppable — and on the transport an operator would filter for. It was
    /// emitted on `nest_rs::layers`, which carries the *layer system's* own
    /// events (a guard declared at two scopes) and nothing else about this
    /// socket: someone tailing `nest_rs::ws=warn` for denials, the way every
    /// other page teaches, saw nothing at all.
    #[tokio::test]
    async fn a_denied_message_warns_on_the_websocket_target() {
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

        assert!(
            frame.contains("is not allowed to post"),
            "the client is told why: {frame}"
        );

        let event = logs.expect_one("nest_rs::ws", "websocket message denied by a guard");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("event").as_deref(), Some("moderated"));
        assert!(
            event.field("reason").is_some_and(|r| r.contains("banned")),
            "the denial reason rides as a field: {event:#?}",
        );
        assert!(
            logs.find("nest_rs::layers", "websocket message denied by a guard")
                .is_empty(),
            "…and only there: {:#?}",
            logs.events(),
        );
    }

    /// The allowed path stays silent on `warn` — a denial log that fires on
    /// every message is as useless as none.
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
        logs.expect_none("nest_rs::ws", "websocket message denied by a guard");
    }

    /// The stalled-outbox close reports, like its two `Closure::Server`
    /// siblings — without it a peer that stopped draining was indistinguishable
    /// from a broken pipe, which is the state [`Closure`]'s own doc says the
    /// close codes exist to end.
    ///
    /// **This pins the event, not the path.** Driving a real socket into a full
    /// outbox needs a client that connects and never reads; what makes the two
    /// agree here is that `stalled_outbox` is the only producer of
    /// `Closure::Server(CloseCode::Policy, OUTBOX_STALLED)` and all three
    /// `try_send` failures route through it.
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
