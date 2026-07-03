use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use nest_rs_core::Container;
use nest_rs_pipes::PipeError;
use poem::web::websocket::{Message, WebSocket};
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response};

use crate::WsReply;

/// Maximum UTF-8 bytes accepted for a single inbound text frame.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
use crate::context::{BoxFuture, Captured, SocketContext};
use crate::envelope::WsEnvelope;
use crate::guard::EventLayerTable;
use crate::server::{Registry, WsClient, WsServer};

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
        let gateway = Arc::clone(&self.gateway);
        let server = Arc::clone(&self.server);
        let guards = Arc::clone(&self.guards);
        let data_pipe = self.data_pipe.clone();
        Ok(ws
            .on_upgrade(move |socket| {
                serve_connection(gateway, server, guards, ambient, data_pipe, socket)
            })
            .into_response())
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
    // Type-erase the registry so `N` never surfaces on `WsClient`.
    let registry: Arc<dyn Registry> = server.clone();
    let client = WsClient::new(conn_id, registry);

    gateway.on_connect(&client).await;

    while let Some(message) = stream.next().await {
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
                    // Replies ride the same outbox as pushes so ordering with
                    // broadcasts the handler triggered is preserved. A full
                    // outbox means the peer is not draining — disconnect it
                    // rather than buffer without bound.
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

    // Fire `on_disconnect` while still registered, then drop.
    gateway.on_disconnect(&client).await;
    server.disconnect(conn_id);
    drop(outbox);
    let _ = writer.await;
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
        WsReply::Reply(data) => serde_json::to_string(&WsEnvelope { event, data }).ok(),
        WsReply::None => None,
        WsReply::Error(message) => Some(error_frame(&event, &message)),
    }
}

fn error_frame(event: &str, message: &str) -> String {
    WsEnvelope::encode(event, &serde_json::json!({ "error": message }))
        .unwrap_or_else(|_| String::from(r#"{"event":"error","data":{"error":"internal"}}"#))
}
