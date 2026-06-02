//! The connection registry and the two handles that read it: [`WsServer`], the
//! `@WebSocketServer` analog — an injectable singleton tracking every live
//! connection (and its rooms) so a service can push to clients beyond the one
//! that spoke — and [`WsClient`], the `@ConnectedSocket` analog handed to a
//! handler so it can address its own socket, a room, or everyone.
//!
//! # Per-gateway namespacing
//!
//! A [`WsServer`] is generic over a zero-sized **namespace** marker `N`
//! (defaulting to [`Global`]). The flat container keys it by type, so
//! `WsServer<Global>` (provided by [`WsModule`]) is one shared registry, while
//! `WsServer<MyNs>` is a wholly separate one: a `broadcast` on the first never
//! reaches the second's clients. A `#[gateway(namespace = MyNs)]` mounts against
//! its own registry (the macro self-provides it), so two gateways isolate without
//! sharing a registry. A handler reaches whichever registry its gateway mounted
//! through `&`[`WsClient`] — and because the client carries the registry as a
//! type-erased [`Registry`], the handler surface (`Gateway`, `MessageGuard`, the
//! lifecycle hooks) stays free of the namespace parameter.
//!
//! [`WsModule`]: crate::WsModule

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use nestrs_core::injectable;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::envelope::WsEnvelope;

/// Identifies one live connection within a [`WsServer`]. Allocated on connect,
/// reclaimed on disconnect; never reused within a process run.
pub type ConnId = u64;

/// The default namespace marker for [`WsServer`] — the single shared registry
/// [`WsModule`] provides and every un-namespaced gateway mounts against.
///
/// [`WsModule`]: crate::WsModule
pub struct Global;

/// One registered connection: the channel that feeds its socket's writer task,
/// plus the rooms it has joined (so a per-connection disconnect drops its room
/// memberships with it).
struct Conn {
    outbox: UnboundedSender<String>,
    rooms: HashSet<String>,
}

/// The connection registry shared across every connection of a gateway — the
/// `@WebSocketServer` analog. Registered as a singleton by [`WsModule`] for the
/// [`Global`] namespace, so any service can `#[inject] server: Arc<WsServer>` and
/// push to clients in reaction to a domain event, not only inside a message
/// handler. A `WsServer<MyNs>` is a distinct registry — see the module docs on
/// per-gateway namespacing.
///
/// [`WsModule`]: crate::WsModule
#[injectable]
pub struct WsServer<N: 'static = Global> {
    conns: Mutex<HashMap<ConnId, Conn>>,
    next: AtomicU64,
    // The namespace marker is type-level only; `fn() -> N` keeps `WsServer<N>`
    // `Send + Sync` without bounding `N`.
    _ns: PhantomData<fn() -> N>,
}

// Manual `Default` (not derived) so it does not spuriously bound `N: Default`.
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
    /// Register a connection's outbox, returning its [`ConnId`]. Called by the
    /// connection loop on upgrade; pairs with [`disconnect`](Self::disconnect).
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

    /// Drop a connection (and all its room memberships). Called when its socket
    /// closes.
    pub(crate) fn disconnect(&self, id: ConnId) {
        self.conns.lock().remove(&id);
    }

    /// Send `data` under `event` to **every** live connection. Returns how many
    /// outboxes accepted the frame; an `Err` means `data` would not serialize
    /// (nothing was sent).
    pub fn broadcast<T: Serialize>(
        &self,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        Ok(self.broadcast_value(event, serde_json::to_value(data)?))
    }

    /// Send `data` under `event` to the connections in `room`. Returns how many
    /// received it.
    pub fn emit_to<T: Serialize>(
        &self,
        room: &str,
        event: &str,
        data: &T,
    ) -> Result<usize, serde_json::Error> {
        Ok(self.emit_to_value(room, event, serde_json::to_value(data)?))
    }

    /// Send `data` under `event` to a single connection. `Ok(false)` means the
    /// connection is gone (or its socket is closing).
    pub fn emit<T: Serialize>(
        &self,
        id: ConnId,
        event: &str,
        data: &T,
    ) -> Result<bool, serde_json::Error> {
        Ok(self.emit_value(id, event, serde_json::to_value(data)?))
    }

    /// Number of live connections — for diagnostics and tests.
    pub fn connection_count(&self) -> usize {
        self.conns.lock().len()
    }
}

/// The non-generic, object-safe face of a [`WsServer`] — the push/room surface a
/// [`WsClient`] needs without naming the namespace. Payloads cross it pre-encoded
/// as [`serde_json::Value`] (the generic encoding happens in the [`WsServer`] /
/// [`WsClient`] convenience methods), so the trait stays object-safe and
/// `WsClient` can hold any namespace's registry as `Arc<dyn Registry>`.
pub trait Registry: Send + Sync + 'static {
    /// Add a connection to a room.
    fn join(&self, id: ConnId, room: &str);
    /// Remove a connection from a room.
    fn leave(&self, id: ConnId, room: &str);
    /// Send a pre-encoded payload to every live connection; returns the count.
    fn broadcast_value(&self, event: &str, data: serde_json::Value) -> usize;
    /// Send a pre-encoded payload to a room; returns the count.
    fn emit_to_value(&self, room: &str, event: &str, data: serde_json::Value) -> usize;
    /// Send a pre-encoded payload to one connection; `false` if it is gone.
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

/// The per-connection handle a `#[subscribe_message]` handler receives by
/// declaring a `&WsClient` parameter — the `@ConnectedSocket` analog. It knows
/// its own [`ConnId`] and shares its gateway's registry as a type-erased
/// [`Registry`], so a handler can reply to itself, manage rooms, or address
/// everyone in its namespace without injecting anything — and without the
/// handler surface naming the namespace.
pub struct WsClient {
    id: ConnId,
    registry: Arc<dyn Registry>,
}

impl WsClient {
    /// Build the handle the connection loop passes into dispatch. Not called by
    /// app code.
    pub fn new(id: ConnId, registry: Arc<dyn Registry>) -> Self {
        Self { id, registry }
    }

    /// A throwaway client backed by a fresh [`WsServer`] and a *closed* outbox
    /// — for unit-testing a `Gateway::dispatch` impl in isolation. The
    /// receiver is dropped immediately, so the registered outbox is closed:
    /// the registry holds a `Sender` whose every `send(...)` call returns
    /// `Err`. As a consequence, [`WsClient::emit`] / [`to`](Self::to) /
    /// [`broadcast`](Self::broadcast) on a `for_test` client report `0` /
    /// `false` (no panic, no leak — frames are silently dropped). For a test
    /// that needs to assert on outbound frames, build the client manually
    /// with a `WsServer` you keep around plus a `Receiver` you read.
    pub fn for_test() -> Self {
        let server: Arc<WsServer> = Arc::new(WsServer::default());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let id = server.connect(tx);
        let registry: Arc<dyn Registry> = server;
        Self { id, registry }
    }

    /// This connection's id.
    pub fn id(&self) -> ConnId {
        self.id
    }

    /// The shared registry, for room-wide or app-wide pushes.
    pub fn registry(&self) -> &Arc<dyn Registry> {
        &self.registry
    }

    /// Join a room — subsequent [`to`](Self::to) calls (from anywhere) reach it.
    pub fn join(&self, room: impl AsRef<str>) {
        self.registry.join(self.id, room.as_ref());
    }

    /// Leave a room.
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

    // A distinct namespace marker keys a wholly separate registry.
    struct OtherNs;

    #[test]
    fn distinct_namespaces_are_independent_registries() {
        let global = WsServer::<Global>::default();
        let other = WsServer::<OtherNs>::default();
        let (tx, mut rx) = unbounded_channel();
        global.connect(tx);

        // A broadcast on the other namespace's registry reaches none of the
        // Global registry's connections.
        assert_eq!(
            Registry::broadcast_value(&other, "ping", serde_json::json!(1)),
            0
        );
        assert_eq!(recv_all(&mut rx).len(), 0);
        assert_eq!(global.broadcast("ping", &1).expect("ok"), 1);
    }
}
