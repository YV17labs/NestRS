use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use poem::web::websocket::{Message, WebSocket};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response};

use crate::context::{BoxFuture, Captured, SocketContext};
use crate::envelope::WsEnvelope;
use crate::guard::MessageGuardTable;
use crate::server::{Registry, WsClient, WsServer};
use crate::WsReply;

/// The per-connection message dispatcher a gateway implements, plus its optional
/// connection lifecycle hooks. `#[messages]` emits this impl: `dispatch` matches
/// the incoming event name against the `#[subscribe_message]` handlers,
/// deserializes the payload, calls the handler (passing the [`WsClient`] to any
/// handler that asks for `&WsClient`), and wraps its return in a [`WsReply`].
/// You never write it by hand.
///
/// [`on_connect`](Self::on_connect) and [`on_disconnect`](Self::on_disconnect)
/// are the `OnGatewayConnection` / `OnGatewayDisconnect` analogs: default no-ops
/// the macro overrides only when the impl block carries an `#[on_connect]` /
/// `#[on_disconnect]` method. The gateway is a singleton shared across every
/// connection, so a hook takes `&self` and is handed the connecting socket's
/// [`WsClient`].
#[async_trait]
pub trait Gateway: Send + Sync + 'static {
    async fn dispatch(&self, client: &WsClient, event: &str, data: serde_json::Value) -> WsReply;

    /// Runs once per connection, right after it registers and before the first
    /// message — the place to join a default room or note presence.
    async fn on_connect(&self, client: &WsClient) {
        let _ = client;
    }

    /// Runs once when the connection loop ends (close frame, transport error, or
    /// a dead writer), while the connection is still registered — so a hook can
    /// still reach the leaving client's rooms before they are dropped.
    async fn on_disconnect(&self, client: &WsClient) {
        let _ = client;
    }
}

/// Build the poem endpoint that upgrades an HTTP request to a WebSocket and runs
/// the gateway's connection loop. Called by the `#[messages]`-generated mount
/// closure with the gateway built once from the container (shared across every
/// connection, like a NestJS gateway singleton), the shared [`WsServer`] registry
/// resolved alongside it, the per-event [`MessageGuardTable`] the macro resolved
/// from the container, and the optional [`SocketContext`] bridge (`None` when no
/// app bound one — the gateway then dispatches with no ambient data context).
pub fn gateway_endpoint<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: MessageGuardTable,
    ctx: Option<Arc<dyn SocketContext>>,
) -> GatewayEndpoint<G, N> {
    GatewayEndpoint {
        gateway,
        server,
        guards: Arc::new(guards),
        ctx,
    }
}

/// The endpoint returned by [`gateway_endpoint`]. Extracts poem's [`WebSocket`]
/// from the request (so a non-upgrade request fails the handshake) and, on
/// upgrade, drives [`serve_connection`]. Generic over the gateway's namespace
/// `N` so it holds (and registers connections into) that gateway's own
/// [`WsServer<N>`]; the namespace never escapes onto the handler surface.
pub struct GatewayEndpoint<G, N: 'static = crate::server::Global> {
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: Arc<MessageGuardTable>,
    /// The per-connection ambient-data bridge, resolved once from the container
    /// at mount. `None` when no app bound one — the connection loop then
    /// dispatches without installing any ambient executor/ability.
    ctx: Option<Arc<dyn SocketContext>>,
}

impl<G: Gateway, N: 'static> Endpoint for GatewayEndpoint<G, N> {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        let (req, mut body) = req.split();
        let ws = WebSocket::from_request(&req, &mut body).await?;
        // Capture the per-connection ambient state *here*, on the post-guard
        // upgrade request — the connection-level guards have already attached the
        // principal/ability to its extensions, and the request does not survive
        // into the connection task that the upgrade spawns. The bridge and its
        // captured state always travel together, so they ride as one `Option`.
        let ambient = self
            .ctx
            .as_ref()
            .map(|ctx| (ctx.clone(), ctx.capture(&req)));
        let gateway = Arc::clone(&self.gateway);
        let server = Arc::clone(&self.server);
        let guards = Arc::clone(&self.guards);
        Ok(ws
            .on_upgrade(move |socket| serve_connection(gateway, server, guards, ambient, socket))
            .into_response())
    }
}

/// Drive one connection. The socket is split so server→client pushes (broadcast,
/// room emits, a handler's own replies) all funnel through one outbox channel
/// drained by a dedicated writer task — decoupling the read/dispatch loop from
/// the single `Sink` and letting [`WsServer`] reach a client it is not currently
/// reading from. The connection registers itself for the duration and is
/// reclaimed when the read loop ends (close frame, transport error, or a dead
/// writer).
async fn serve_connection<G: Gateway, N: 'static>(
    gateway: Arc<G>,
    server: Arc<WsServer<N>>,
    guards: Arc<MessageGuardTable>,
    ambient: Option<(Arc<dyn SocketContext>, Captured)>,
    socket: poem::web::websocket::WebSocketStream,
) {
    let (mut sink, mut stream) = socket.split();
    let (outbox, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // The writer owns the sink and forwards every queued text frame until the
    // channel closes (connection ending) or the socket errors.
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink.send(Message::Text(frame)).await.is_err() {
                break;
            }
        }
    });

    let conn_id = server.connect(outbox.clone());
    // Hand the client the registry type-erased, so the namespace `N` never
    // surfaces on the handler-facing `WsClient`.
    let registry: Arc<dyn Registry> = server.clone();
    let client = WsClient::new(conn_id, registry);

    // The connection is live and registered: fire the `on_connect` hook before
    // the first message so it can join a room or note presence.
    gateway.on_connect(&client).await;

    while let Some(message) = stream.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if let Some(reply) =
                    handle_text(&*gateway, &guards, ambient.as_ref(), &client, &text).await
                {
                    // A handler's direct reply rides the same outbox as a push,
                    // so ordering relative to broadcasts the handler triggered is
                    // preserved. A closed channel means the writer is gone.
                    if outbox.send(reply).is_err() {
                        break;
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            // Binary/Ping/Pong are not part of the JSON envelope protocol yet.
            Ok(_) => {}
            Err(err) => {
                tracing::debug!(target: "nestrs::ws", error = %err, "websocket read error");
                break;
            }
        }
    }

    // Fire `on_disconnect` while the connection is still registered, then drop it.
    gateway.on_disconnect(&client).await;
    server.disconnect(conn_id);
    // Drop our outbox so the writer's channel closes and the task ends, then
    // await it so the sink is flushed/closed before we return.
    drop(outbox);
    let _ = writer.await;
}

/// Parse one text frame as an envelope, run its event's per-message guards, and
/// — if they pass — dispatch it (handing the handler its [`WsClient`]), rendering
/// the reply frame (or `None` for a `()`-returning handler). A guard rejection
/// short-circuits to an error frame under the request's event name; the handler
/// never runs.
///
/// Per-message guards run **inside** a present [`SocketContext`]'s
/// [`around`](SocketContext::around), so they see the same ambient task-locals
/// the handler will (the request executor, the caller's ability). That is what
/// lets a marker like `WsAuthGuard` reject a mis-wired gateway that dropped its
/// connection-level auth guard — without the captured ability installed,
/// `nestrs_authz::current_ability()` is `None` and the marker fails closed. The
/// no-context path runs guards then the handler bare, so the existing message-
/// level guards keep working.
async fn handle_text<G: Gateway>(
    gateway: &G,
    guards: &MessageGuardTable,
    ambient: Option<&(Arc<dyn SocketContext>, Captured)>,
    client: &WsClient,
    text: &str,
) -> Option<String> {
    let envelope: WsEnvelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(err) => return Some(error_frame("error", &format!("invalid envelope: {err}"))),
    };
    let WsEnvelope { event, data } = envelope;
    // Bundle guard check + dispatch into one `BoxFuture` so `around` wraps both —
    // a guard that reads `current_ability()` / `current_executor()` therefore
    // sees the captured state, exactly like the handler does.
    let event_ref = event.clone();
    let inner: BoxFuture<'_, WsReply> = Box::pin(async move {
        if let Err(reason) = guards.check(client, &event_ref, &data).await {
            return WsReply::Error(reason);
        }
        gateway.dispatch(client, &event_ref, data).await
    });
    let reply = match ambient {
        Some((ctx, captured)) => ctx.around(captured, inner).await,
        None => inner.await,
    };
    match reply {
        WsReply::Reply(data) => serde_json::to_string(&WsEnvelope { event, data }).ok(),
        WsReply::None => None,
        WsReply::Error(message) => Some(error_frame(&event, &message)),
    }
}

/// Render an error reply frame: the request's event name with `data: { error }`.
fn error_frame(event: &str, message: &str) -> String {
    WsEnvelope::encode(event, &serde_json::json!({ "error": message }))
        .unwrap_or_else(|_| String::from(r#"{"event":"error","data":{"error":"internal"}}"#))
}
