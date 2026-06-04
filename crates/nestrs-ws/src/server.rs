//! Connection registry and the two handles that read it: [`WsServer`] (the
//! `@WebSocketServer` analog, an injectable singleton tracking every live
//! connection) and [`WsClient`] (the `@ConnectedSocket` analog handed to a
//! handler).
//!
//! [`WsServer`] is generic over a zero-sized namespace marker `N`
//! (default [`Global`]). The flat container keys by type, so `WsServer<Global>`
//! and `WsServer<MyNs>` are wholly separate registries — `#[gateway(namespace
//! = MyNs)]` mounts against its own, self-provided registry. [`WsClient`]
//! holds the registry as a type-erased [`Registry`] so the handler surface
//! stays free of the namespace parameter.
//!
//! [`WsModule`]: crate::WsModule

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use nestrs_core::injectable;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::envelope::WsEnvelope;

/// Identifies one live connection within a [`WsServer`]. Allocated on connect;
/// never reused within a process run.
pub type ConnId = u64;

/// Default namespace marker for [`WsServer`].
pub struct Global;

struct Conn {
    outbox: UnboundedSender<String>,
    rooms: HashSet<String>,
}

/// Connection registry shared across every connection of a gateway — the
/// `@WebSocketServer` analog. Registered as a singleton by [`WsModule`] for
/// the [`Global`] namespace; any service can `#[inject] Arc<WsServer>` to
/// push to clients in reaction to a domain event.
///
/// [`WsModule`]: crate::WsModule
#[injectable]
pub struct WsServer<N: 'static = Global> {
    conns: Mutex<HashMap<ConnId, Conn>>,
    next: AtomicU64,
    // `fn() -> N` keeps `WsServer<N>: Send + Sync` without bounding `N`.
    _ns: PhantomData<fn() -> N>,
}

// Manual `Default` so `N: Default` is not required.
impl<N: 'static> Default for WsServer<N> {
    fn default() -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            _ns: PhantomData,
        }
    }
}

impl<N: 'static> WsServer<N> {
    pub(crate) fn connect(&self, outbox: UnboundedSender<String>) -> ConnId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.conns.lock().insert(
            id,
            Conn {
                outbox,
                rooms: HashSet::new(),
            },
        );
        id
    }

    pub(crate) fn disconnect(&self, id: ConnId) {
        self.conns.lock().remove(&id);
    }

    /// Send `data` under `event` to every live connection. Returns the number
    /// of outboxes that accepted the frame.
    pub fn broadcast<T: Serialize>(
        &self,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        Ok(self.broadcast_value(event, serde_json::to_value(data)?))
    }

    /// Send `data` under `event` to connections in `room`.
    pub fn emit_to<T: Serialize>(
        &self,
        room: &str,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        Ok(self.emit_to_value(room, event, serde_json::to_value(data)?))
    }

    /// Send `data` under `event` to a single connection. `Ok(false)` if the
    /// connection is gone.
    pub fn emit<T: Serialize>(
        &self,
        id: ConnId,
        event: &str,
        data: &T,
    ) -> Result<bool, serde_json::Error> {
        Ok(self.emit_value(id, event, serde_json::to_value(data)?))
    }

    pub fn connection_count(&self) -> usize {
        self.conns.lock().len()
    }
}

/// Object-safe face of a [`WsServer`] — the push/room surface a [`WsClient`]
/// needs without naming the namespace. Payloads cross it pre-encoded as
/// [`serde_json::Value`] so the trait stays object-safe.
pub trait Registry: Send + Sync + 'static {
    fn join(&self, id: ConnId, room: &str);
    fn leave(&self, id: ConnId, room: &str);
    fn broadcast_value(&self, event: &str, data: serde_json::Value) -> usize;
    fn emit_to_value(&self, room: &str, event: &str, data: serde_json::Value) -> usize;
    fn emit_value(&self, id: ConnId, event: &str, data: serde_json::Value) -> bool;
}

impl<N: 'static> Registry for WsServer<N> {
    fn join(&self, id: ConnId, room: &str) {
        if let Some(conn) = self.conns.lock().get_mut(&id) {
            conn.rooms.insert(room.to_owned());
        }
    }

    fn leave(&self, id: ConnId, room: &str) {
        if let Some(conn) = self.conns.lock().get_mut(&id) {
            conn.rooms.remove(room);
        }
    }

    fn broadcast_value(&self, event: &str, data: serde_json::Value) -> usize {
        let Ok(frame) = WsEnvelope::encode(event, &data) else {
            return 0;
        };
        let conns = self.conns.lock();
        conns
            .values()
            .filter(|conn| conn.outbox.send(frame.clone()).is_ok())
            .count()
    }

    fn emit_to_value(&self, room: &str, event: &str, data: serde_json::Value) -> usize {
        let Ok(frame) = WsEnvelope::encode(event, &data) else {
            return 0;
        };
        let conns = self.conns.lock();
        conns
            .values()
            .filter(|conn| conn.rooms.contains(room))
            .filter(|conn| conn.outbox.send(frame.clone()).is_ok())
            .count()
    }

    fn emit_value(&self, id: ConnId, event: &str, data: serde_json::Value) -> bool {
        let Ok(frame) = WsEnvelope::encode(event, &data) else {
            return false;
        };
        let conns = self.conns.lock();
        conns
            .get(&id)
            .is_some_and(|conn| conn.outbox.send(frame).is_ok())
    }
}

/// Per-connection handle a `#[subscribe_message]` handler receives by
/// declaring a `&WsClient` parameter — the `@ConnectedSocket` analog. Holds
/// its gateway's registry as a type-erased [`Registry`] so the handler
/// surface stays free of the namespace parameter.
pub struct WsClient {
    id: ConnId,
    registry: Arc<dyn Registry>,
}

impl WsClient {
    pub fn new(id: ConnId, registry: Arc<dyn Registry>) -> Self {
        Self { id, registry }
    }

    /// Throwaway client backed by a fresh [`WsServer`] and a closed outbox —
    /// for unit-testing `Gateway::dispatch` in isolation. Sends silently
    /// drop (return `0` / `false`). Tests asserting on outbound frames must
    /// build the client manually with a kept `Receiver`.
    pub fn for_test() -> Self {
        let server: Arc<WsServer> = Arc::new(WsServer::default());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let id = server.connect(tx);
        let registry: Arc<dyn Registry> = server;
        Self { id, registry }
    }

    pub fn id(&self) -> ConnId {
        self.id
    }

    pub fn registry(&self) -> &Arc<dyn Registry> {
        &self.registry
    }

    pub fn join(&self, room: impl AsRef<str>) {
        self.registry.join(self.id, room.as_ref());
    }

    pub fn leave(&self, room: &str) {
        self.registry.leave(self.id, room);
    }

    /// Send `data` under `event` to this connection only.
    pub fn emit<T: Serialize>(&self, event: &str, data: &T) -> Result<bool, serde_json::Error> {
        Ok(self
            .registry
            .emit_value(self.id, event, serde_json::to_value(data)?))
    }

    /// Send `data` under `event` to a room.
    pub fn to<T: Serialize>(
        &self,
        room: &str,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        Ok(self
            .registry
            .emit_to_value(room, event, serde_json::to_value(data)?))
    }

    /// Send `data` under `event` to every connection (including this one).
    pub fn broadcast<T: Serialize>(
        &self,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        Ok(self
            .registry
            .broadcast_value(event, serde_json::to_value(data)?))
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
        let server = WsServer::<Global>::default();
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
        let server = WsServer::<Global>::default();
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

    struct OtherNs;

    #[test]
    fn distinct_namespaces_are_independent_registries() {
        let global = WsServer::<Global>::default();
        let other = WsServer::<OtherNs>::default();
        let (tx, mut rx) = unbounded_channel();
        global.connect(tx);

        assert_eq!(
            Registry::broadcast_value(&other, "ping", serde_json::json!(1)),
            0
        );
        assert_eq!(recv_all(&mut rx).len(), 0);
        assert_eq!(global.broadcast("ping", &1).expect("ok"), 1);
    }
}
