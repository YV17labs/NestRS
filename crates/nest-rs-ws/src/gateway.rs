use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use nest_rs_core::{Container, RequestScope};
use nest_rs_pipes::PipeError;
use poem::web::websocket::{Message, WebSocket};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response};

use crate::WsReply;

/// Maximum UTF-8 bytes accepted for a single inbound text frame.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
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
    async fn dispatch(&self, client: &WsClient, event: &str, data: serde_json::Value) -> WsReply;

    async fn on_connect(&self, client: &WsClient) {
        let _ = client;
    }

    /// Runs while the connection is still registered, so a hook can reach the
    /// leaving client's rooms before they are dropped.
    async fn on_disconnect(&self, client: &WsClient) {
        let _ = client;
    }
}

/// A per-message data-pipe runner with the container already captured, so
/// [`handle_text`] (which has no container) can fold the global pipes over a
/// message's `data`. Built at mount by [`resolve_ws_data_pipe`].
pub type WsDataFold = dyn Fn(&str, &mut serde_json::Value) -> Result<(), PipeError> + Send + Sync;

/// Bridge slot for global pipes on a WS message's `data` — the per-message
/// analog of HTTP's `transform_body`. `nest-rs-guards`' `use_pipes_global`
/// provides a fn pointer that folds every registered global pipe's
/// [`GlobalPipe::transform_ws_data`](nest_rs_pipes::GlobalPipe) over the data.
/// Defined here (the gateway calls it), provided by guards (which owns the
/// `PipeSpecs` registry) — the same seeded-fn-pointer pattern as the GraphQL
/// `GraphqlVariablePipe`, since guards depends on this crate, not the reverse.
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

pub fn gateway_endpoint<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: EventLayerTable,
    ctx: Option<Arc<dyn SocketContext>>,
    data_pipe: Option<Arc<WsDataFold>>,
) -> GatewayEndpoint<G, N> {
    GatewayEndpoint {
        gateway,
        server,
        guards: Arc::new(guards),
        ctx,
        data_pipe,
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
        // Resolve the socket-lifetime ceiling once per upgrade from the request
        // scope the HTTP transport installs. A missing scope or unregistered
        // `WsConfig` falls back to the (bounded) default — fail-secure, never a
        // silently unbounded socket.
        let max_lifetime = req
            .extensions()
            .get::<Arc<RequestScope>>()
            .and_then(|scope| scope.root().get::<WsConfig>())
            .unwrap_or_default()
            .max_connection;
        let gateway = Arc::clone(&self.gateway);
        let server = Arc::clone(&self.server);
        let guards = Arc::clone(&self.guards);
        let data_pipe = self.data_pipe.clone();
        Ok(ws
            .on_upgrade(move |socket| {
                serve_connection(gateway, server, guards, ambient, data_pipe, max_lifetime, socket)
            })
            .into_response())
    }
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
async fn serve_connection<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: Arc<EventLayerTable>,
    ambient: Option<(Arc<dyn SocketContext>, Captured)>,
    data_pipe: Option<Arc<WsDataFold>>,
    max_lifetime: Option<Duration>,
    socket: poem::web::websocket::WebSocketStream,
) {
    let (mut sink, mut stream) = socket.split();
    let (outbox, mut rx) = tokio::sync::mpsc::channel::<String>(crate::server::OUTBOX_CAPACITY);

    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink.send(Message::Text(frame)).await.is_err() {
                break;
            }
        }
    });

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

    gateway.on_connect(&client).await;

    // Optional socket-lifetime ceiling. When it elapses the server closes the
    // socket through the same path as a client `Close`, so a principal captured
    // once at the upgrade cannot outlive the ceiling (bounding the stale-privilege
    // window after token expiry/logout/revocation). `None` ⇒ unlimited, modeled
    // as an inert `select!` arm so an unbounded socket runs exactly as before.
    let mut lifetime = max_lifetime.map(|ttl| Box::pin(tokio::time::sleep(ttl)));

    loop {
        tokio::select! {
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
                    target: "nest_rs::ws",
                    conn_id,
                    "closing socket: max lifetime reached",
                );
                break;
            }
            message = stream.next() => {
                let Some(message) = message else { break };
                match message {
                    Ok(Message::Text(text)) => {
                        if text.len() > MAX_MESSAGE_BYTES {
                            let frame = error_frame("error", "message too large");
                            if outbox.try_send(frame).is_err() {
                                break;
                            }
                            continue;
                        }
                        if let Some(reply) = handle_text(
                            &*gateway,
                            &guards,
                            ambient.as_ref(),
                            data_pipe.as_ref(),
                            &client,
                            &text,
                        )
                        .await
                        {
                            // Replies ride the same outbox as pushes so ordering
                            // with broadcasts the handler triggered is preserved.
                            // A full outbox means the peer is not draining —
                            // disconnect it rather than buffer without bound.
                            if outbox.try_send(reply).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(err) => {
                        tracing::debug!(target: "nest_rs::ws", error = %err, "websocket read error");
                        break;
                    }
                }
            }
        }
    }

    // Fire `on_disconnect` while still registered, then drop the guard to
    // remove the entry. Dropping it *before* awaiting the writer releases the
    // registry's outbox `Sender` clone so the writer task observes the channel
    // close; on an unwind the guard's `Drop` does the same cleanup.
    gateway.on_disconnect(&client).await;
    drop(registry_guard);
    drop(outbox);
    // A `JoinError` from the writer means it panicked (it is never aborted);
    // surface that rather than swallow it. A normal cancellation carries none.
    if let Err(err) = writer.await
        && err.is_panic()
    {
        tracing::warn!(target: "nest_rs::ws", error = %err, "writer task failed");
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
    ambient: Option<&(Arc<dyn SocketContext>, Captured)>,
    data_pipe: Option<&Arc<WsDataFold>>,
    client: &WsClient,
    text: &str,
) -> Option<String> {
    let envelope: WsEnvelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(_) => return Some(error_frame("error", "invalid envelope")),
    };
    let WsEnvelope { event, mut data } = envelope;
    // Bundle guard + dispatch so `around` wraps both — a guard reading
    // `current_ability()` / `current_executor()` sees the captured state.
    let event_ref = event.clone();
    let inner: BoxFuture<'_, WsReply> = Box::pin(async move {
        if let Err(reason) = guards.check(client, &event_ref, &data).await {
            return WsReply::Error(reason);
        }
        // Global data pipes run after guards (which see the raw value), before
        // dispatch — the per-message analog of HTTP running pipes after guards.
        if let Some(pipe) = data_pipe
            && let Err(err) = pipe(&event_ref, &mut data)
        {
            return WsReply::error(format!("invalid data for `{event_ref}`: {}", err.message()));
        }
        gateway.dispatch(client, &event_ref, data).await
    });
    let reply = match ambient {
        Some((ctx, captured)) => ctx.around(captured, inner).await,
        None => inner.await,
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
                        target: "nest_rs::ws",
                        event = %envelope.event,
                        error = %err,
                        "failed to serialize reply",
                    );
                    Some(error_frame(&envelope.event, "internal error"))
                }
            }
        }
        WsReply::None => None,
        WsReply::Error(message) => Some(error_frame(&event, &message)),
    }
}

fn error_frame(event: &str, message: &str) -> String {
    WsEnvelope::encode(event, &serde_json::json!({ "error": message }))
        .unwrap_or_else(|_| String::from(r#"{"event":"error","data":{"error":"internal"}}"#))
}
