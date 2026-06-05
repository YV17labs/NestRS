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

    #[test]
    fn emit_to_a_specific_connection_lands_on_that_inbox_only() {
        let server = WsServer::<Global>::default();
        let (tx_a, mut rx_a) = unbounded_channel();
        let (tx_b, mut rx_b) = unbounded_channel();
        let a = server.connect(tx_a);
        let _b = server.connect(tx_b);

        assert!(server.emit(a, "hello", &"hi").expect("serializes"));
        assert_eq!(recv_all(&mut rx_a).len(), 1);
        assert_eq!(recv_all(&mut rx_b).len(), 0);
    }

    #[test]
    fn emit_to_an_unknown_connection_returns_false() {
        let server = WsServer::<Global>::default();
        // 99 was never connected.
        let id: ConnId = 99;
        assert!(!server.emit(id, "x", &"y").expect("serializes"));
    }

    #[test]
    fn emit_to_an_empty_room_sends_zero_frames() {
        let server = WsServer::<Global>::default();
        let (tx, mut rx) = unbounded_channel();
        let _ = server.connect(tx);
        assert_eq!(server.emit_to("ghost", "x", &"y").expect("serializes"), 0);
        assert!(recv_all(&mut rx).is_empty());
    }

    #[test]
    fn connection_count_tracks_connects_and_disconnects() {
        let server = WsServer::<Global>::default();
        assert_eq!(server.connection_count(), 0);

        let (tx_a, _rx_a) = unbounded_channel();
        let a = server.connect(tx_a);
        assert_eq!(server.connection_count(), 1);

        let (tx_b, _rx_b) = unbounded_channel();
        let _b = server.connect(tx_b);
        assert_eq!(server.connection_count(), 2);

        server.disconnect(a);
        assert_eq!(server.connection_count(), 1);
    }

    #[test]
    fn ws_client_join_leave_routes_through_the_registry() {
        let server = Arc::new(WsServer::<Global>::default());
        let (tx_a, mut rx_a) = unbounded_channel();
        let (tx_b, mut rx_b) = unbounded_channel();
        let a = server.connect(tx_a);
        let _b = server.connect(tx_b);

        let registry: Arc<dyn Registry> = server.clone();
        let client = WsClient::new(a, registry);
        client.join("lobby");

        assert_eq!(server.emit_to("lobby", "msg", &1).expect("ok"), 1);
        assert_eq!(recv_all(&mut rx_a).len(), 1);
        assert_eq!(recv_all(&mut rx_b).len(), 0);

        client.leave("lobby");
        assert_eq!(server.emit_to("lobby", "msg", &2).expect("ok"), 0);
    }

    #[test]
    fn ws_client_emit_sends_to_its_own_connection_only() {
        let server = Arc::new(WsServer::<Global>::default());
        let (tx_a, mut rx_a) = unbounded_channel();
        let (tx_b, mut rx_b) = unbounded_channel();
        let a = server.connect(tx_a);
        let _b = server.connect(tx_b);

        let registry: Arc<dyn Registry> = server;
        let client = WsClient::new(a, registry);
        assert!(client.emit("ping", &"hi").expect("serializes"));
        assert_eq!(recv_all(&mut rx_a).len(), 1);
        assert!(recv_all(&mut rx_b).is_empty());
    }

    #[test]
    fn ws_client_broadcast_reaches_every_connection_including_self() {
        let server = Arc::new(WsServer::<Global>::default());
        let (tx_a, mut rx_a) = unbounded_channel();
        let (tx_b, mut rx_b) = unbounded_channel();
        let a = server.connect(tx_a);
        let _b = server.connect(tx_b);

        let registry: Arc<dyn Registry> = server;
        let client = WsClient::new(a, registry);
        assert_eq!(client.broadcast("hi", &"all").expect("serializes"), 2);
        assert_eq!(recv_all(&mut rx_a).len(), 1);
        assert_eq!(recv_all(&mut rx_b).len(), 1);
    }

    #[test]
    fn ws_client_to_emits_into_the_named_room_only() {
        // `WsClient::to(room, event, data)` is the `@ConnectedSocket.to` analog —
        // sends to peers in `room` regardless of whether `self` joined it.
        let server = Arc::new(WsServer::<Global>::default());
        let (tx_a, mut rx_a) = unbounded_channel();
        let (tx_b, mut rx_b) = unbounded_channel();
        let a = server.connect(tx_a);
        let b = server.connect(tx_b);
        server.join(b, "lobby");

        let registry: Arc<dyn Registry> = server.clone();
        let client = WsClient::new(a, registry);
        let count = client.to("lobby", "msg", &"hi").expect("serializes");

        assert_eq!(count, 1, "only the room member receives the frame");
        assert!(recv_all(&mut rx_a).is_empty());
        assert_eq!(recv_all(&mut rx_b).len(), 1);
    }

    #[test]
    fn ws_client_to_an_empty_room_returns_zero() {
        let server = Arc::new(WsServer::<Global>::default());
        let (tx, _rx) = unbounded_channel();
        let id = server.connect(tx);
        let registry: Arc<dyn Registry> = server;
        let client = WsClient::new(id, registry);
        // No-one joined "ghost" — the count is zero, no error.
        assert_eq!(client.to("ghost", "msg", &"hi").expect("serializes"), 0);
    }

    #[test]
    fn ws_client_id_returns_the_allocated_connection_id() {
        // The `id()` accessor is part of the handler-facing surface — gateways
        // store it to keep per-connection state.
        let server = Arc::new(WsServer::<Global>::default());
        let (tx, _rx) = unbounded_channel();
        let assigned = server.connect(tx);
        let registry: Arc<dyn Registry> = server;
        let client = WsClient::new(assigned, registry);
        assert_eq!(client.id(), assigned);
    }

    #[test]
    fn ws_client_registry_accessor_returns_the_underlying_registry() {
        // Handlers reach `WsServer` through this dyn handle without naming `N`.
        let server = Arc::new(WsServer::<Global>::default());
        let (tx, mut rx) = unbounded_channel();
        let id = server.connect(tx);
        let registry: Arc<dyn Registry> = server.clone();
        let client = WsClient::new(id, registry);

        // Calling `broadcast_value` through the accessor reaches the same
        // backing server.
        let sent = client
            .registry()
            .broadcast_value("evt", serde_json::json!({"k": 1}));
        assert_eq!(sent, 1);
        assert_eq!(recv_all(&mut rx).len(), 1);
    }

    #[test]
    fn registry_dyn_dispatch_routes_join_and_leave_through_the_namespace() {
        // The trait object is what `WsClient` actually holds — verify each
        // method dispatches to the underlying `WsServer<N>` impl.
        let server: Arc<dyn Registry> = Arc::new(WsServer::<Global>::default());
        // Build a real connection on the underlying server through the trait —
        // we can't call `connect` through `dyn Registry`, so reach the impl.
        // Instead, exercise the four message-routing methods on an unknown id /
        // unknown room: each should return the documented "no-op" value.
        Registry::join(&*server, 999, "room");
        Registry::leave(&*server, 999, "room");
        assert_eq!(
            Registry::broadcast_value(&*server, "x", serde_json::json!(1)),
            0,
            "no connections ⇒ zero frames",
        );
        assert_eq!(
            Registry::emit_to_value(&*server, "room", "x", serde_json::json!(1)),
            0,
            "empty room ⇒ zero frames",
        );
        assert!(
            !Registry::emit_value(&*server, 999, "x", serde_json::json!(1)),
            "unknown connection ⇒ false",
        );
    }

    #[test]
    fn ws_server_with_a_custom_namespace_carries_its_own_connections() {
        // Tag `WsServer<MyNs>` with a non-`Default` marker — covers the
        // namespace-typed path and the manual `Default` impl that does not
        // bound `N: Default`.
        struct MyNs;
        let server = WsServer::<MyNs>::default();
        let (tx, mut rx) = unbounded_channel();
        let id = server.connect(tx);
        assert_eq!(server.connection_count(), 1);

        assert!(server.emit(id, "ping", &"hi").expect("serializes"));
        assert_eq!(recv_all(&mut rx).len(), 1);

        server.disconnect(id);
        assert_eq!(server.connection_count(), 0);
    }

    #[test]
    fn ws_client_for_test_yields_a_dropable_outbox() {
        // `for_test` is the documented shim for unit-testing gateway handlers
        // without a real server — sends are accepted (registry exists) but
        // the rx is dropped so frames are silently shed.
        let client = WsClient::for_test();
        // `emit` writes to a closed channel — registry's send returns false,
        // but the constructor doesn't panic and the public API stays usable.
        let _ = client.emit("hello", &"world");
        // `id()` and `registry()` are usable trivial accessors.
        let _ = client.id();
        let _ = client.registry();
    }
}
