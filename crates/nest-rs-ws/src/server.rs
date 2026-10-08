//! Connection registry and the two handles that read it: [`WsServer`], an
//! injectable singleton per namespace marker tracking every live connection, and
//! [`WsClient`], handed to a handler.

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use nest_rs_core::injectable;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc::Sender;

use crate::envelope::WsEnvelope;

/// Identifies one live connection within a [`WsServer`]. Allocated on connect;
/// never reused within a process run.
pub type ConnId = u64;

/// Bounded per-connection outbox capacity: a slow consumer sheds further pushes
/// rather than growing memory without bound.
pub(crate) const OUTBOX_CAPACITY: usize = 256;

/// Default namespace marker for [`WsServer`].
pub struct Global;

/// An encoded outbound frame, shared so a broadcast hands the same bytes to
/// every recipient.
pub(crate) type Frame = Arc<str>;

struct Conn {
    outbox: Sender<Frame>,
    rooms: HashSet<String>,
}

/// Connection registry shared across every connection of a gateway, provided
/// by [`WsModule`]; any service can `#[inject] Arc<WsServer>` to push to clients.
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
    pub(crate) fn connect(&self, outbox: Sender<Frame>) -> ConnId {
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

    /// Snapshot the outboxes of the connections matching `select`, releasing
    /// the registry lock before anything is pushed — see [`fan_out`].
    fn recipients(&self, select: impl Fn(&Conn) -> bool) -> Vec<Sender<Frame>> {
        let conns = self.conns.lock();
        let mut out = Vec::with_capacity(conns.len());
        out.extend(
            conns
                .values()
                .filter(|conn| select(conn))
                .map(|conn| conn.outbox.clone()),
        );
        out
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

    /// Number of live connections currently registered — for health/metrics.
    pub fn connection_count(&self) -> usize {
        self.conns.lock().len()
    }
}

/// Object-safe face of a [`WsServer`] — the push/room surface a [`WsClient`]
/// needs without naming the namespace. Payloads cross it pre-encoded as
/// [`serde_json::Value`] so the trait stays object-safe.
pub trait Registry: Send + Sync + 'static {
    /// Add a connection to a room (idempotent; no-op if the id is gone).
    fn join(&self, id: ConnId, room: &str);
    /// Remove a connection from a room (no-op if absent).
    fn leave(&self, id: ConnId, room: &str);
    /// Send a pre-encoded frame to every connection; returns the count reached.
    fn broadcast_value(&self, event: &str, data: serde_json::Value) -> usize;
    /// Send a pre-encoded frame to every connection in `room`; returns the count
    /// reached.
    fn emit_to_value(&self, room: &str, event: &str, data: serde_json::Value) -> usize;
    /// Send a pre-encoded frame to one connection; `false` if it is gone.
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
        let (sent, shed) = fan_out(self.recipients(|_| true), frame.into());
        warn_if_shed(event, "broadcast", sent, shed);
        sent
    }

    fn emit_to_value(&self, room: &str, event: &str, data: serde_json::Value) -> usize {
        let Ok(frame) = WsEnvelope::encode(event, &data) else {
            return 0;
        };
        let recipients = self.recipients(|conn| conn.rooms.contains(room));
        let (sent, shed) = fan_out(recipients, frame.into());
        warn_if_shed(event, "room", sent, shed);
        sent
    }

    fn emit_value(&self, id: ConnId, event: &str, data: serde_json::Value) -> bool {
        let Ok(frame) = WsEnvelope::encode(event, &data) else {
            return false;
        };
        let conns = self.conns.lock();
        let sent = conns
            .get(&id)
            .is_some_and(|conn| conn.outbox.try_send(frame.into()).is_ok());
        // A registered connection that couldn't take the frame has a full outbox.
        if conns.contains_key(&id) && !sent {
            warn_if_shed(event, "direct", 0, 1);
        }
        sent
    }
}

/// Push one frame to each recipient, returning `(sent, shed)`. Takes a snapshot
/// so the registry lock is never held across the push loop.
fn fan_out(recipients: Vec<Sender<Frame>>, frame: Frame) -> (usize, usize) {
    let (mut sent, mut shed) = (0usize, 0usize);
    for outbox in recipients {
        if outbox.try_send(Arc::clone(&frame)).is_ok() {
            sent += 1;
        } else {
            shed += 1;
        }
    }
    (sent, shed)
}

/// Surface shed frames at `warn`, one aggregated event per send call. A shed
/// frame is dropped, not retried.
fn warn_if_shed(event: &str, kind: &'static str, sent: usize, shed: usize) {
    if shed > 0 {
        tracing::warn!(
            target: crate::TARGET,
            event,
            kind,
            sent,
            shed,
            "server→client frames shed: recipient outbox full (slow/dead client)",
        );
    }
}

/// Per-connection handle a `#[subscribe_message]` handler receives by
/// declaring a `&WsClient` parameter. Holds its gateway's registry as a
/// type-erased [`Registry`], free of the namespace parameter.
pub struct WsClient {
    id: ConnId,
    registry: Arc<dyn Registry>,
}

impl WsClient {
    /// Build a client handle over a connection id and its gateway's registry.
    pub fn new(id: ConnId, registry: Arc<dyn Registry>) -> Self {
        Self { id, registry }
    }

    /// Throwaway client backed by a fresh [`WsServer`] and a closed outbox, for
    /// unit-testing `Gateway::dispatch`. Sends drop (return `0` / `false`).
    pub fn for_test() -> Self {
        let server: Arc<WsServer> = Arc::new(WsServer::default());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let id = server.connect(tx);
        let registry: Arc<dyn Registry> = server;
        Self { id, registry }
    }

    /// This connection's stable id, for addressing it later via the registry.
    pub fn id(&self) -> ConnId {
        self.id
    }

    /// The type-erased [`Registry`] backing this client — for broadcasts or
    /// room fan-out beyond this single connection.
    pub fn registry(&self) -> &Arc<dyn Registry> {
        &self.registry
    }

    /// Add this connection to `room`, so a later `emit_to`/`to` reaches it.
    pub fn join(&self, room: impl AsRef<str>) {
        self.registry.join(self.id, room.as_ref());
    }

    /// Remove this connection from `room`.
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
    use tokio::sync::mpsc::Receiver;

    /// Test outbox: matches the production bounded channel.
    fn unbounded_channel() -> (Sender<Frame>, Receiver<Frame>) {
        tokio::sync::mpsc::channel(OUTBOX_CAPACITY)
    }

    fn recv_all(rx: &mut Receiver<Frame>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            out.push(frame.to_string());
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
        assert_eq!(client.to("ghost", "msg", &"hi").expect("serializes"), 0);
    }

    #[test]
    fn ws_client_id_returns_the_allocated_connection_id() {
        let server = Arc::new(WsServer::<Global>::default());
        let (tx, _rx) = unbounded_channel();
        let assigned = server.connect(tx);
        let registry: Arc<dyn Registry> = server;
        let client = WsClient::new(assigned, registry);
        assert_eq!(client.id(), assigned);
    }

    #[test]
    fn ws_client_registry_accessor_returns_the_underlying_registry() {
        let server = Arc::new(WsServer::<Global>::default());
        let (tx, mut rx) = unbounded_channel();
        let id = server.connect(tx);
        let registry: Arc<dyn Registry> = server.clone();
        let client = WsClient::new(id, registry);

        let sent = client
            .registry()
            .broadcast_value("evt", serde_json::json!({"k": 1}));
        assert_eq!(sent, 1);
        assert_eq!(recv_all(&mut rx).len(), 1);
    }

    #[test]
    fn registry_dyn_dispatch_routes_join_and_leave_through_the_namespace() {
        let server: Arc<dyn Registry> = Arc::new(WsServer::<Global>::default());
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
        // A non-`Default` marker covers the manual `Default` impl.
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
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test proves an emit into a dropped outbox does not panic; its result is not the point"
    )]
    fn ws_client_for_test_yields_a_dropable_outbox() {
        let client = WsClient::for_test();
        let _ = client.emit("hello", &"world");
        let _ = client.id();
        let _ = client.registry();
    }

    #[test]
    fn a_full_outbox_reports_the_frames_it_dropped_rather_than_losing_them_silently() {
        let logs = nest_rs_testing::LogCapture::install();
        let server = WsServer::<Global>::default();
        let (tx, _rx) = tokio::sync::mpsc::channel::<Frame>(1);
        let id = server.connect(tx.clone());
        tx.try_send(Frame::from("occupied"))
            .expect("the first frame fits");

        assert!(
            !server
                .emit(id, "tick", &"payload")
                .expect("the payload serializes"),
            "a full outbox sheds the frame",
        );

        let event = logs.expect_one(
            "nest_rs::ws",
            "server→client frames shed: recipient outbox full (slow/dead client)",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("event").as_deref(), Some("tick"));
        assert_eq!(event.field("kind").as_deref(), Some("direct"));
        assert_eq!(event.field("shed").as_deref(), Some("1"));
    }
}
