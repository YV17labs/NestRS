//! The connection registry and the two handles that read it: [`WsServer`], the
//! `@WebSocketServer` analog — an injectable singleton tracking every live
//! connection (and its rooms) so a service can push to clients beyond the one
//! that spoke — and [`WsClient`], the `@ConnectedSocket` analog handed to a
//! handler so it can address its own socket, a room, or everyone.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nestrs_core::injectable;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::envelope::WsEnvelope;

/// Identifies one live connection within a [`WsServer`]. Allocated on connect,
/// reclaimed on disconnect; never reused within a process run.
pub type ConnId = u64;

/// One registered connection: the channel that feeds its socket's writer task,
/// plus the rooms it has joined (so a per-connection disconnect drops its room
/// memberships with it).
struct Conn {
    outbox: UnboundedSender<String>,
    rooms: HashSet<String>,
}

/// The connection registry shared across every connection of a gateway — the
/// `@WebSocketServer` analog. Registered as a singleton by [`WsModule`], so any
/// service can `#[inject] server: Arc<WsServer>` and push to clients in reaction
/// to a domain event, not only inside a message handler.
///
/// [`WsModule`]: crate::WsModule
#[injectable]
#[derive(Default)]
pub struct WsServer {
    conns: Mutex<HashMap<ConnId, Conn>>,
    next: AtomicU64,
}

impl WsServer {
    /// Register a connection's outbox, returning its [`ConnId`]. Called by the
    /// connection loop on upgrade; pairs with [`disconnect`](Self::disconnect).
    pub(crate) fn connect(&self, outbox: UnboundedSender<String>) -> ConnId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.conns.lock().unwrap().insert(
            id,
            Conn {
                outbox,
                rooms: HashSet::new(),
            },
        );
        id
    }

    /// Drop a connection (and all its room memberships). Called when its socket
    /// closes.
    pub(crate) fn disconnect(&self, id: ConnId) {
        self.conns.lock().unwrap().remove(&id);
    }

    /// Add a connection to a room. A later [`emit_to`](Self::emit_to) reaches it;
    /// a no-op if the connection has already left.
    pub fn join(&self, id: ConnId, room: impl Into<String>) {
        if let Some(conn) = self.conns.lock().unwrap().get_mut(&id) {
            conn.rooms.insert(room.into());
        }
    }

    /// Remove a connection from a room.
    pub fn leave(&self, id: ConnId, room: &str) {
        if let Some(conn) = self.conns.lock().unwrap().get_mut(&id) {
            conn.rooms.remove(room);
        }
    }

    /// Send `data` under `event` to **every** live connection. Returns how many
    /// outboxes accepted the frame; an `Err` means `data` would not serialize
    /// (nothing was sent).
    pub fn broadcast<T: Serialize>(
        &self,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        let frame = WsEnvelope::encode(event, data)?;
        let conns = self.conns.lock().unwrap();
        let sent = conns
            .values()
            .filter(|conn| conn.outbox.send(frame.clone()).is_ok())
            .count();
        Ok(sent)
    }

    /// Send `data` under `event` to the connections in `room`. Returns how many
    /// received it.
    pub fn emit_to<T: Serialize>(
        &self,
        room: &str,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        let frame = WsEnvelope::encode(event, data)?;
        let conns = self.conns.lock().unwrap();
        let sent = conns
            .values()
            .filter(|conn| conn.rooms.contains(room))
            .filter(|conn| conn.outbox.send(frame.clone()).is_ok())
            .count();
        Ok(sent)
    }

    /// Send `data` under `event` to a single connection. `Ok(false)` means the
    /// connection is gone (or its socket is closing).
    pub fn emit<T: Serialize>(
        &self,
        id: ConnId,
        event: &str,
        data: &T,
    ) -> Result<bool, serde_json::Error> {
        let frame = WsEnvelope::encode(event, data)?;
        let conns = self.conns.lock().unwrap();
        Ok(conns
            .get(&id)
            .is_some_and(|conn| conn.outbox.send(frame).is_ok()))
    }

    /// Number of live connections — for diagnostics and tests.
    pub fn connection_count(&self) -> usize {
        self.conns.lock().unwrap().len()
    }
}

/// The per-connection handle a `#[subscribe_message]` handler receives by
/// declaring a `&WsClient` parameter — the `@ConnectedSocket` analog. It knows
/// its own [`ConnId`] and shares the gateway's [`WsServer`], so a handler can
/// reply to itself, manage rooms, or address everyone without injecting anything.
pub struct WsClient {
    id: ConnId,
    server: Arc<WsServer>,
}

impl WsClient {
    /// Build the handle the connection loop passes into dispatch. Not called by
    /// app code.
    pub fn new(id: ConnId, server: Arc<WsServer>) -> Self {
        Self { id, server }
    }

    /// This connection's id.
    pub fn id(&self) -> ConnId {
        self.id
    }

    /// The shared registry, for room-wide or app-wide pushes.
    pub fn server(&self) -> &Arc<WsServer> {
        &self.server
    }

    /// Join a room — subsequent [`to`](Self::to) calls (from anywhere) reach it.
    pub fn join(&self, room: impl Into<String>) {
        self.server.join(self.id, room);
    }

    /// Leave a room.
    pub fn leave(&self, room: &str) {
        self.server.leave(self.id, room);
    }

    /// Send `data` under `event` to this connection only.
    pub fn emit<T: Serialize>(&self, event: &str, data: &T) -> Result<bool, serde_json::Error> {
        self.server.emit(self.id, event, data)
    }

    /// Send `data` under `event` to a room.
    pub fn to<T: Serialize>(
        &self,
        room: &str,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        self.server.emit_to(room, event, data)
    }

    /// Send `data` under `event` to every connection (including this one).
    pub fn broadcast<T: Serialize>(
        &self,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        self.server.broadcast(event, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn recv_all(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            out.push(frame);
        }
        out
    }

    #[test]
    fn broadcast_reaches_every_connection() {
        let server = WsServer::default();
        let (tx_a, mut rx_a) = unbounded_channel();
        let (tx_b, mut rx_b) = unbounded_channel();
        server.connect(tx_a);
        server.connect(tx_b);

        let sent = server.broadcast("ping", &"hi").expect("serializes");

        assert_eq!(sent, 2);
        assert_eq!(recv_all(&mut rx_a).len(), 1);
        assert_eq!(recv_all(&mut rx_b).len(), 1);
    }

    #[test]
    fn emit_to_scopes_by_room_and_disconnect_clears_membership() {
        let server = WsServer::default();
        let (tx_a, mut rx_a) = unbounded_channel();
        let (tx_b, mut rx_b) = unbounded_channel();
        let a = server.connect(tx_a);
        let b = server.connect(tx_b);
        server.join(a, "lobby");

        assert_eq!(server.emit_to("lobby", "msg", &1).expect("ok"), 1);
        assert_eq!(recv_all(&mut rx_a).len(), 1);
        assert_eq!(recv_all(&mut rx_b).len(), 0);

        server.disconnect(b);
        assert_eq!(server.connection_count(), 1);
    }
}
