use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use poem::web::websocket::{Message, WebSocket, WebSocketStream};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response};

use crate::envelope::WsEnvelope;
use crate::WsReply;

/// The per-connection message dispatcher a gateway implements. `#[messages]`
/// emits this impl: `dispatch` matches the incoming event name against the
/// `#[subscribe_message]` handlers, deserializes the payload, calls the handler,
/// and wraps its return in a [`WsReply`]. You never write it by hand.
#[async_trait]
pub trait Gateway: Send + Sync + 'static {
    async fn dispatch(&self, event: &str, data: serde_json::Value) -> WsReply;
}

/// Build the poem endpoint that upgrades an HTTP request to a WebSocket and runs
/// the gateway's connection loop. Called by the `#[messages]`-generated mount
/// closure with the gateway built once from the container (shared across every
/// connection, like a NestJS gateway singleton).
pub fn gateway_endpoint<G: Gateway>(gateway: Arc<G>) -> GatewayEndpoint<G> {
    GatewayEndpoint { gateway }
}

/// The endpoint returned by [`gateway_endpoint`]. Extracts poem's [`WebSocket`]
/// from the request (so a non-upgrade request fails the handshake) and, on
/// upgrade, drives [`serve_connection`].
pub struct GatewayEndpoint<G> {
    gateway: Arc<G>,
}

impl<G: Gateway> Endpoint for GatewayEndpoint<G> {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        let (req, mut body) = req.split();
        let ws = WebSocket::from_request(&req, &mut body).await?;
        let gateway = Arc::clone(&self.gateway);
        Ok(ws
            .on_upgrade(move |socket| serve_connection(gateway, socket))
            .into_response())
    }
}

/// Read text frames off the socket until it closes, dispatching each through the
/// gateway and writing back any reply. A send failure or a transport error ends
/// the connection.
async fn serve_connection<G: Gateway>(gateway: Arc<G>, mut socket: WebSocketStream) {
    while let Some(message) = socket.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if let Some(reply) = handle_text(&*gateway, &text).await {
                    if socket.send(Message::Text(reply)).await.is_err() {
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
}

/// Parse one text frame as an envelope, dispatch it, and render the reply frame
/// (or `None` for a `()`-returning handler).
async fn handle_text<G: Gateway>(gateway: &G, text: &str) -> Option<String> {
    let envelope: WsEnvelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(err) => return Some(error_frame("error", &format!("invalid envelope: {err}"))),
    };
    let event = envelope.event;
    match gateway.dispatch(&event, envelope.data).await {
        WsReply::Reply(data) => serde_json::to_string(&WsEnvelope { event, data }).ok(),
        WsReply::None => None,
        WsReply::Error(message) => Some(error_frame(&event, &message)),
    }
}

/// Render an error reply frame: the request's event name with `data: { error }`.
fn error_frame(event: &str, message: &str) -> String {
    serde_json::to_string(&WsEnvelope {
        event: event.to_string(),
        data: serde_json::json!({ "error": message }),
    })
    .unwrap_or_else(|_| String::from(r#"{"event":"error","data":{"error":"internal"}}"#))
}
