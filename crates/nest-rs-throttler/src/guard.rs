//! [`ThrottlerGuard`] — rate-limiting guard.

use std::fmt::{self, Write as _};
use std::future::{Future as _, poll_fn};
use std::net::{IpAddr, Ipv6Addr};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::HandlerMetadata;
use nest_rs_http::{ClientOrigin, Reflector, async_trait};
use poem::Request;

#[cfg(feature = "graphql")]
use nest_rs_graphql::GraphqlOperationContext;
#[cfg(feature = "mcp")]
use nest_rs_mcp::McpOperationContext;
#[cfg(feature = "ws")]
use nest_rs_ws::WsClient;

use crate::pseudonym::PseudonymStore;
use crate::store::{Decision, HIT_TIMEOUT, ThrottlerStore};
use crate::throttle::Throttle;

/// The edge a bucket belongs to — the leading segment of every key, and the
/// `transport` field on every denial: a `#[query]`, a `#[tool]` and a
/// `#[subscribe_message]` may share a name, and must not share a budget.
mod transport {
    pub(super) const HTTP: &str = "http";
    #[cfg(feature = "graphql")]
    pub(super) const GRAPHQL: &str = "graphql";
    #[cfg(feature = "mcp")]
    pub(super) const MCP: &str = "mcp";
    #[cfg(feature = "ws")]
    pub(super) const WS: &str = "ws";
}

/// U+001F (unit separator) joins the parts of a bucket key: it appears in none
/// of them, so a composite key never collides across the join.
const KEY_SEPARATOR: char = '\u{1f}';

/// The sentence a throttled caller reads, whichever edge refused it.
const RATE_LIMITED_MESSAGE: &str = "Too Many Requests";

/// The key `parts` address, joined by [`KEY_SEPARATOR`].
fn bucket_key(parts: &[&dyn fmt::Display]) -> String {
    let mut key = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            key.push(KEY_SEPARATOR);
        }
        #[expect(
            clippy::let_underscore_must_use,
            reason = "fmt::Write for String never fails"
        )]
        let _ = write!(key, "{part}");
    }
    key
}

/// The wait a caller is told to observe, in whole seconds — **rounded up, and
/// never `0`**.
///
/// Truncating would hand every denial in a window's last second
/// `Retry-After: 0`, which RFC 9110 §10.2.3 reads as "retry immediately".
fn retry_after_secs(retry_after: Duration) -> u32 {
    let secs = retry_after
        .as_secs()
        .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
        .max(1);
    u32::try_from(secs).unwrap_or(u32::MAX)
}

/// The refusal every edge returns.
fn rate_limited(retry_after: Duration) -> Denial {
    Denial::rate_limited(retry_after_secs(retry_after), RATE_LIMITED_MESSAGE)
}

/// Counts one unit of work against a limit and refuses the caller over it —
/// `429` + `Retry-After` on HTTP, the edge's own error frame elsewhere.
///
/// **All four request-carrying edges.** The bucket is the unit the edge
/// *addresses*, joined with the caller that edge can see: the matched route
/// pattern and the client's network on HTTP — its address in IPv4, its `/64`
/// in IPv6 — the field name on GraphQL, the tool or prompt on MCP, the event
/// and the connection on WS.
///
/// **Only HTTP carries per-unit metadata**, so `#[meta(Throttle::...)]`
/// overrides the module default there and nowhere else; the other edges count
/// against [`ThrottlerConfig`](crate::ThrottlerConfig)'s limit.
///
/// The store is the bound `Arc<dyn ThrottlerStore>`:
/// [`InMemoryThrottler`](crate::InMemoryThrottler) by default, or a shared
/// store when its module is imported instead.
#[injectable]
pub struct ThrottlerGuard {
    /// The limit a route that pins no `#[meta(Throttle)]` runs under.
    /// **Injected, never defaulted**: a guard built without `for_root` fails the
    /// boot naming `Throttle` rather than run 60/minute in silence.
    #[inject]
    default: Arc<Throttle>,
    /// The store, as [`ThrottlerGuard::new`] prepared it. No provider registers
    /// a [`CountingStore`], so a guard built by any path but `new` fails the
    /// boot rather than hand a store outside the process the client's address.
    #[inject]
    throttler: Arc<CountingStore>,
}

/// The store a guard counts in: the bound store itself when its counters stay
/// in this process, behind its pseudonyms otherwise ([`PseudonymStore::wrap`]).
pub(crate) struct CountingStore(Arc<dyn ThrottlerStore>);

impl ThrottlerGuard {
    /// Build the guard over a store, with the default limit for routes that pin
    /// none, counting under pseudonyms when the store's counters leave this
    /// process ([`PseudonymStore::wrap`] — refused without `pseudonym_key`).
    pub(crate) fn new(
        throttler: Arc<dyn ThrottlerStore>,
        default: Throttle,
        pseudonym_key: Option<&str>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            default: Arc::new(default),
            throttler: Arc::new(CountingStore(PseudonymStore::wrap(
                throttler,
                pseudonym_key,
            )?)),
        })
    }

    /// Count one hit for `key` under `limit` on the edge `transport` names,
    /// waiting on the store no longer than [`HIT_TIMEOUT`].
    ///
    /// A store silent past the bound is denied for the whole window, fail
    /// closed. The call is polled once bare before the bound is armed, so the
    /// in-process store pays no timer.
    async fn count(&self, transport: &'static str, key: &str, limit: Throttle) -> Decision {
        let store = &*self.throttler.0;
        let mut hit = pin!(store.hit(key, limit));
        if let Poll::Ready(decision) = poll_fn(|cx| Poll::Ready(hit.as_mut().poll(cx))).await {
            return decision;
        }
        match tokio::time::timeout(HIT_TIMEOUT, hit).await {
            Ok(decision) => decision,
            Err(_) => {
                tracing::warn!(
                    target: crate::TARGET,
                    transport,
                    store = ThrottlerStore::name(store),
                    waited_ms = u64::try_from(HIT_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                    "throttler store did not answer within the guard's timeout; denying \
                     (fail-closed)",
                );
                Decision::denied(limit.window())
            }
        }
    }
}

impl Layer for ThrottlerGuard {}

#[async_trait]
impl Guard for ThrottlerGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let limit = Reflector::new(req)
            .get::<Throttle>()
            .copied()
            .unwrap_or(*self.default);

        // Per route: keyed on the IP alone, a lenient route would drain a strict
        // route's budget.
        let ip = ClientId::from(ClientOrigin::of(req));
        // The declared template, so dynamic segments don't fragment the bucket;
        // a request the router matched nothing for falls back to its raw path.
        let route = nest_rs_http::__private::matched_template(req)
            .unwrap_or_else(|| req.uri().path().into());
        let key = bucket_key(&[&transport::HTTP, &route, &ip]);

        let decision = self.count(transport::HTTP, &key, limit).await;
        if decision.allowed {
            return Ok(());
        }
        // The route alone, never the store key: the key holds the client's
        // address, personal data no line carries.
        tracing::warn!(
            target: crate::TARGET,
            transport = transport::HTTP,
            route = %route,
            retry_after = retry_after_secs(decision.retry_after),
            "rate limit exceeded",
        );
        Err(rate_limited(decision.retry_after))
    }

    /// GraphQL's unit is the **field**, counted once per field resolved.
    ///
    /// **The caller half is the actor**
    /// ([`current_actor_id`](nest_rs_core::current_actor_id)): the peer address
    /// is not reachable here, and keyed on the field alone one client would
    /// `429` everybody. An **anonymous** caller shares one bucket, reported once
    /// per process; the same guard in `use_guards_global` meters `/graphql` per
    /// client address.
    #[cfg(feature = "graphql")]
    async fn check_graphql(&self, operation: &GraphqlOperationContext<'_>) -> Result<(), Denial> {
        let field = operation.name();
        static SEEN: AtomicBool = AtomicBool::new(false);
        let caller = caller_bucket(
            &SEEN,
            "graphql_anonymous_operation_shares_a_bucket",
            "an anonymous GraphQL operation has no actor to key on, so every anonymous caller \
             shares one bucket per field; bind ThrottlerGuard in use_guards_global as well, so \
             the /graphql request itself is metered per client address",
        );
        let key = bucket_key(&[&transport::GRAPHQL, &field, &caller]);

        let decision = self.count(transport::GRAPHQL, &key, *self.default).await;
        if decision.allowed {
            return Ok(());
        }
        tracing::warn!(
            target: crate::TARGET,
            transport = transport::GRAPHQL,
            operation = %field,
            retry_after = retry_after_secs(decision.retry_after),
            "rate limit exceeded",
        );
        Err(rate_limited(decision.retry_after))
    }

    /// MCP's unit is the **operation** — the tool or prompt the client named.
    /// The kind leads the name because the protocol namespaces them separately:
    /// a `#[tool]` and a `#[prompt]` may share a name and are two addresses.
    ///
    /// The caller half is the actor, as on GraphQL: `nest_rs_mcp::propagate`
    /// carries the request scope across the task rmcp spawns.
    #[cfg(feature = "mcp")]
    async fn check_mcp(&self, ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        let kind = ctx.kind();
        let name = ctx.name();
        static SEEN: AtomicBool = AtomicBool::new(false);
        let caller = caller_bucket(
            &SEEN,
            "mcp_anonymous_operation_shares_a_bucket",
            "an anonymous MCP operation has no actor to key on, so every anonymous caller shares \
             one bucket per operation; bind ThrottlerGuard in use_guards_global as well, so the \
             /mcp request itself is metered per client address",
        );
        let key = bucket_key(&[&transport::MCP, &kind, &name, &caller]);

        let decision = self.count(transport::MCP, &key, *self.default).await;
        if decision.allowed {
            return Ok(());
        }
        tracing::warn!(
            target: crate::TARGET,
            transport = transport::MCP,
            kind = %kind,
            operation = %name,
            retry_after = retry_after_secs(decision.retry_after),
            "rate limit exceeded",
        );
        Err(rate_limited(decision.retry_after))
    }

    /// WS's unit is the **event**, and the caller half the connection: the peer
    /// address is gone by dispatch. A reconnect opens a fresh bucket, which the
    /// same guard on the `#[gateway]` struct meters per address at the upgrade.
    #[cfg(feature = "ws")]
    async fn check_ws_message(
        &self,
        client: &WsClient,
        event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), Denial> {
        let connection = client.id();
        let key = bucket_key(&[&transport::WS, &event, &connection]);

        let decision = self.count(transport::WS, &key, *self.default).await;
        if decision.allowed {
            return Ok(());
        }
        tracing::warn!(
            target: crate::TARGET,
            transport = transport::WS,
            event = %event,
            connection,
            retry_after = retry_after_secs(decision.retry_after),
            "rate limit exceeded",
        );
        Err(rate_limited(decision.retry_after))
    }
}

/// `ThrottlerGuard` checks HTTP requests: [`check_http`](Guard::check_http)
/// counts one request against its route's bucket. Declared so a
/// `#[controller]`, a `#[routes]` verb or a `#[gateway]` struct may bind it.
impl nest_rs_guards::HttpGuard for ThrottlerGuard {}

/// …and GraphQL operations, keyed on the field
/// ([`check_graphql`](Guard::check_graphql)). Declared so a `#[resolver]` or a
/// single `#[query]` may bind it.
#[cfg(feature = "graphql")]
impl nest_rs_guards::GraphqlGuard for ThrottlerGuard {}

/// …and MCP operations, keyed on the tool or prompt
/// ([`check_mcp`](Guard::check_mcp)). Declared so an `#[mcp]` host or a single
/// `#[tool]` may bind it.
#[cfg(feature = "mcp")]
impl nest_rs_guards::McpGuard for ThrottlerGuard {}

/// …and WS messages, keyed on the event and the connection
/// ([`check_ws_message`](Guard::check_ws_message)). Declared so a
/// `#[subscribe_message]` may bind it; on the `#[gateway]` struct the marker
/// required is `HttpGuard`, because those guards run on the upgrade.
#[cfg(feature = "ws")]
impl nest_rs_guards::WsGuard for ThrottlerGuard {}

/// The identity a rate-limit bucket is keyed on.
enum ClientId {
    /// The network the resolved address belongs to, with its prefix length: the
    /// address itself in IPv4 (`/32`), its `/64` in IPv6 — the link one host's
    /// addresses share (RFC 4291 §2.5.4) while it rotates the rest (RFC 8981).
    Network(IpAddr, u8),
    /// No address could be resolved — every caller shares one bucket. See
    /// [`warn_shared_bucket`].
    Shared,
}

impl ClientId {
    /// The network `ip` is counted under. An IPv4 client a dual-stack socket
    /// reports as `::ffff:a.b.c.d` is counted as IPv4: its `/64` would be every
    /// IPv4 client at once.
    fn network(ip: IpAddr) -> Self {
        match ip.to_canonical() {
            IpAddr::V4(v4) => Self::Network(IpAddr::V4(v4), 32),
            IpAddr::V6(v6) => Self::Network(
                IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & !u128::from(u64::MAX))),
                64,
            ),
        }
    }
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(ip, prefix) => write!(f, "{ip}/{prefix}"),
            Self::Shared => f.write_str("global"),
        }
    }
}

/// Turn the transport's [`ClientOrigin`] into a bucket key, warning on the two
/// resolutions that collapse every caller into one bucket.
impl From<ClientOrigin> for ClientId {
    fn from(origin: ClientOrigin) -> Self {
        match origin {
            ClientOrigin::Peer(ip) | ClientOrigin::Forwarded(ip) => Self::network(ip),
            ClientOrigin::TrustedProxy(ip) => {
                static SEEN: AtomicBool = AtomicBool::new(false);
                warn_shared_bucket(
                    &SEEN,
                    "trusted_proxy_without_forwarded_for",
                    "the direct peer is a trusted proxy but sent no usable X-Forwarded-For — \
                     every caller behind it shares one rate-limit bucket; make the proxy forward \
                     the client address",
                );
                Self::network(ip)
            }
            ClientOrigin::Unknown => {
                static SEEN: AtomicBool = AtomicBool::new(false);
                warn_shared_bucket(
                    &SEEN,
                    "no_peer_address",
                    "no peer address (unix socket, or a proxy that hides it) — every caller \
                     shares one rate-limit bucket, so a single client can exhaust the budget for \
                     all of them",
                );
                Self::Shared
            }
        }
    }
}

/// The caller half of an in-band bucket key: the authenticated principal, or a
/// reported shared bucket when there is none.
#[cfg(any(feature = "graphql", feature = "mcp"))]
fn caller_bucket(seen: &AtomicBool, reason: &'static str, detail: &'static str) -> String {
    match nest_rs_core::current_actor_id() {
        Some(actor) => actor,
        None => {
            warn_shared_bucket(seen, reason, detail);
            ANONYMOUS_CALLER.to_owned()
        }
    }
}

/// What an unauthenticated caller is keyed as; never `""`, which an actor could
/// be named.
#[cfg(any(feature = "graphql", feature = "mcp"))]
const ANONYMOUS_CALLER: &str = "<anonymous>";

/// Report a keying degradation **once per process**, at `warn`.
///
/// One flag per reason, from the call site, rather than a `Mutex` on a path
/// every anonymous in-band request takes.
fn warn_shared_bucket(seen: &AtomicBool, reason: &'static str, detail: &'static str) {
    if !seen.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            target: crate::TARGET,
            reason,
            detail,
            "rate-limit keying degraded to a shared bucket",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    use super::*;
    use crate::DEFAULT_THROTTLE;

    /// A store whose counters leave this process, recording every key it is
    /// handed.
    #[derive(Default)]
    struct Recording(parking_lot::Mutex<Vec<String>>);

    #[async_trait]
    impl ThrottlerStore for Recording {
        async fn hit(&self, key: &str, _limit: Throttle) -> Decision {
            self.0.lock().push(key.to_owned());
            Decision::allowed()
        }
    }

    /// A request from `peer`, which poem's builder cannot set.
    fn from_peer(peer: &str) -> Request {
        use poem::Addr;
        use poem::web::{LocalAddr, RemoteAddr};

        let socket: SocketAddr = format!("{peer}:54321").parse().expect("test literal");
        let (parts, ()) = poem::http::Request::new(()).into_parts();
        Request::from_parts(
            (
                parts,
                LocalAddr::default(),
                RemoteAddr(Addr::socket(socket)),
                poem::http::uri::Scheme::HTTP,
            )
                .into(),
            poem::Body::empty(),
        )
    }

    const KEY: &str = "a pseudonym key of at least 32 bytes, for tests";

    #[tokio::test]
    async fn a_client_address_never_reaches_a_store_outside_the_process() {
        let store = Arc::new(Recording::default());
        let guard = ThrottlerGuard::new(store.clone(), DEFAULT_THROTTLE, Some(KEY))
            .expect("a store outside the process boots with a key");
        for peer in ["203.0.113.7", "203.0.113.7", "203.0.113.8"] {
            guard
                .check_http(&mut from_peer(peer))
                .await
                .expect("a hit under the limit is allowed");
        }
        let keys = store.0.lock();
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert!(
            keys.iter()
                .all(|key| !key.contains("203.0.113") && !key.contains('/')),
            "{keys:?}"
        );
        assert_eq!(keys[0], keys[1], "one client, one bucket");
        assert_ne!(keys[0], keys[2], "two clients, two buckets");
    }

    #[test]
    fn a_store_outside_the_process_without_a_key_is_refused() {
        let Err(refused) =
            ThrottlerGuard::new(Arc::new(Recording::default()), DEFAULT_THROTTLE, None)
        else {
            panic!("a store outside the process must not run without a pseudonym key");
        };
        let said = refused.to_string();
        assert!(
            said.contains(&nest_rs_config::var_name("throttler", "PSEUDONYM_KEY"))
                && said.contains("Recording"),
            "{said}"
        );
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test literal is an IP")
    }

    // The resolution itself, spoofing cases included, is tested with
    // `nest_rs_http::ClientOrigin`; here only its mapping onto a bucket.
    #[test]
    fn every_resolved_address_gets_its_own_bucket() {
        let addr = ip("203.0.113.50");
        assert_eq!(
            ClientId::from(ClientOrigin::Peer(addr)).to_string(),
            "203.0.113.50/32"
        );
        assert_eq!(
            ClientId::from(ClientOrigin::Forwarded(addr)).to_string(),
            "203.0.113.50/32",
            "a hop a trusted proxy forwarded keys the same as a direct peer",
        );
    }

    #[test]
    fn ipv6_keys_on_its_64_and_ipv4_on_its_address() {
        let id = |addr: &str| ClientId::from(ClientOrigin::Peer(ip(addr))).to_string();
        assert_eq!(id("2001:db8:1:2::1"), "2001:db8:1:2::/64");
        assert_eq!(
            id("2001:db8:1:2::1"),
            id("2001:db8:1:2:ffff:ffff:ffff:fffe")
        );
        assert_ne!(id("2001:db8:1:2::1"), id("2001:db8:1:3::1"));
        assert_eq!(id("::ffff:203.0.113.7"), id("203.0.113.7"));
        assert_ne!(id("203.0.113.7"), id("203.0.113.8"));
    }

    #[test]
    fn a_trusted_proxy_that_forwards_nothing_keys_on_the_proxy() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            ClientId::from(ClientOrigin::TrustedProxy(proxy)).to_string(),
            "10.0.0.1/32",
        );
    }

    #[test]
    fn no_peer_address_falls_back_to_a_named_global_bucket() {
        assert_eq!(ClientId::from(ClientOrigin::Unknown).to_string(), "global");
    }

    /// Deduped once per process, which nextest's process per test makes safe to
    /// assert on.
    #[test]
    fn a_degraded_keying_is_reported_once_with_its_remedy() {
        let logs = nest_rs_testing::LogCapture::install();
        assert_eq!(
            ClientId::from(ClientOrigin::Unknown).to_string(),
            ClientId::Shared.to_string(),
        );

        let event = logs.expect_one(
            crate::TARGET,
            "rate-limit keying degraded to a shared bucket",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("reason").as_deref(), Some("no_peer_address"));
        assert!(
            event
                .field("detail")
                .is_some_and(|d| d.contains("shares one rate-limit bucket")),
            "the remedy is the point of the line, got {:?}",
            event.fields,
        );

        #[expect(
            clippy::let_underscore_must_use,
            reason = "the call is made for the line it would emit a second time"
        )]
        let _ = ClientId::from(ClientOrigin::Unknown);
        assert_eq!(
            logs.find(
                crate::TARGET,
                "rate-limit keying degraded to a shared bucket",
            )
            .len(),
            1,
            "reported once per process, per reason",
        );
    }
}
