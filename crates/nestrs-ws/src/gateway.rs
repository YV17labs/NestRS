use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use poem::web::websocket::{Message, WebSocket};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response};

use crate::envelope::WsEnvelope;
use crate::server::{WsClient, WsServer};
use crate::WsReply;

/// The per-connection message dispatcher a gateway implements. `#[messages]`
/// emits this impl: `dispatch` matches the incoming event name against the
/// `#[subscribe_message]` handlers, deserializes the payload, calls the handler
/// (passing the [`WsClient`] to any handler that asks for `&WsClient`), and wraps
/// its return in a [`WsReply`]. You never write it by hand.
#[async_trait]
pub trait Gateway: Send + Sync + 'static {
    async fn dispatch(&self, client: &WsClient, event: &str, data: serde_json::Value) -> WsReply;
}

/// Build the poem endpoint that upgrades an HTTP request to a WebSocket and runs
/// the gateway's connection loop. Called by the `#[messages]`-generated mount
/// closure with the gateway built once from the container (shared across every
/// connection, like a NestJS gateway singleton) and the shared [`WsServer`]
/// registry resolved alongside it.
pub fn gateway_endpoint<G: Gateway>(gateway: Arc<G>, server: Arc<WsServer>) -> GatewayEndpoint<G> {
    GatewayEndpoint { gateway, server }
}

/// The endpoint returned by [`gateway_endpoint`]. Extracts poem's [`WebSocket`]
/// from the request (so a non-upgrade request fails the handshake) and, on
/// upgrade, drives [`serve_connection`].
pub struct GatewayEndpoint<G> {
    gateway: Arc<G>,
    server: Arc<WsServer>,
}

impl<G: Gateway> Endpoint for GatewayEndpoint<G> {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        let (req, mut body) = req.split();
        let ws = WebSocket::from_request(&req, &mut body).await?;
        let gateway = Arc::clone(&self.gateway);
        let server = Arc::clone(&self.server);
        Ok(ws
            .on_upgrade(move |socket| serve_connection(gateway, server, socket))
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
async fn serve_connection<G: Gateway>(
    gateway: Arc<G>,
    server: Arc<WsServer>,
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
    let client = WsClient::new(conn_id, Arc::clone(&server));

    while let Some(message) = stream.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if let Some(reply) = handle_text(&*gateway, &client, &text).await {
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

    server.disconnect(conn_id);
    // Drop our outbox so the writer's channel closes and the task ends, then
    // await it so the sink is flushed/closed before we return.
    drop(outbox);
    let _ = writer.await;
}

/// Parse one text frame as an envelope, dispatch it (handing the handler its
/// [`WsClient`]), and render the reply frame (or `None` for a `()`-returning
/// handler).
async fn handle_text<G: Gateway>(gateway: &G, client: &WsClient, text: &str) -> Option<String> {
    let envelope: WsEnvelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(err) => return Some(error_frame("error", &format!("invalid envelope: {err}"))),
    };
    let event = envelope.event;
    match gateway.dispatch(client, &event, envelope.data).await {
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
