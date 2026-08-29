//! [`ThrottlerGuard`] — rate-limiting guard.

use std::fmt::{self, Write as _};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::HandlerMetadata;
use nest_rs_http::{ClientOrigin, Reflector, async_trait};
use poem::{PathPattern, Request};

#[cfg(feature = "graphql")]
use nest_rs_graphql::GraphqlOperationContext;
#[cfg(feature = "mcp")]
use nest_rs_mcp::McpOperationContext;
#[cfg(feature = "ws")]
use nest_rs_ws::WsClient;

use crate::store::ThrottlerStore;
use crate::throttle::Throttle;

/// The edge a bucket belongs to — the leading segment of every key, and the
/// `transport` field on every denial.
///
/// One store serves all four edges and the units they address share a namespace
/// of bare names: a `#[query]`, a `#[tool]` and a `#[subscribe_message]` may all
/// be called `search`. Without this segment they would drain one budget between
/// them, so a client could exhaust a tool's window by spamming a socket.
mod transport {
    pub(super) const HTTP: &str = "http";
    #[cfg(feature = "graphql")]
    pub(super) const GRAPHQL: &str = "graphql";
    #[cfg(feature = "mcp")]
    pub(super) const MCP: &str = "mcp";
    #[cfg(feature = "ws")]
    pub(super) const WS: &str = "ws";
}

/// U+001F (unit separator) joins the parts of a bucket key. It can appear in
/// none of them — a route pattern, a GraphQL field, an MCP operation name, a WS
/// event, an IP, a connection id — so a composite key never collides across the
/// join.
const KEY_SEPARATOR: char = '\u{1f}';

/// The sentence a throttled caller reads, whichever edge refused it. One
/// vocabulary across an HTTP body, a GraphQL error frame, an MCP error and a WS
/// error frame, because a client speaking two of them must not read two.
const RATE_LIMITED_MESSAGE: &str = "Too Many Requests";

/// The key `parts` address, joined by [`KEY_SEPARATOR`].
fn bucket_key(parts: &[&dyn fmt::Display]) -> String {
    let mut key = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            key.push(KEY_SEPARATOR);
        }
        // Writing to a `String` cannot fail; `fmt::Write` still returns a
        // `Result`, and this is a hot path that owes no `expect`.
        let _ = write!(key, "{part}");
    }
    key
}

/// The wait a caller is told to observe, in whole seconds — **rounded up, and
/// never `0`**.
///
/// Both stores compute a sub-second remainder (the in-memory one from
/// `window - elapsed`, the Redis one from the key's real TTL), so truncating
/// hands every denial in the final second of a window `Retry-After: 0`. RFC 9110
/// §10.2.3 reads that as "retry immediately", which turns the refusal into an
/// instruction to hot-retry against the limit the guard just enforced — the load
/// it exists to shed. `nest_rs_http`'s shed-load `503` states the same rule for
/// the same reason; this is the throttler's half of it.
///
/// Read once here and used twice — the denial the caller receives and the
/// `warn` the operator reads — so the header and the log line cannot disagree.
fn retry_after_secs(retry_after: Duration) -> u32 {
    let secs = retry_after
        .as_secs()
        .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
        .max(1);
    u32::try_from(secs).unwrap_or(u32::MAX)
}

/// The refusal every edge returns, so the wait and the sentence a caller reads
/// cannot drift between transports.
fn rate_limited(retry_after: Duration) -> Denial {
    Denial::rate_limited(retry_after_secs(retry_after), RATE_LIMITED_MESSAGE)
}

/// Counts one unit of work against a limit and refuses the caller over it —
/// `429` + `Retry-After` on HTTP, the edge's own error frame elsewhere.
///
/// **All four request-carrying edges.** The bucket is the unit the edge
/// *addresses*, joined with the caller that edge can see: the matched route
/// pattern and the client address on HTTP, the field name on GraphQL, the tool
/// or prompt on MCP, the event and the connection on WS. Three of them are not
/// reachable through the HTTP chain at all — `/graphql` and `/mcp` are
/// [`EdgePosture::Exempt`](nest_rs_http::EdgePosture), and a WS message runs
/// after the upgrade has returned — so a guard that only checked HTTP left them
/// unmetered at every binding scope.
///
/// **Only HTTP carries per-unit metadata**, so `#[meta(Throttle::...)]`
/// overrides the module default there and nowhere else: a GraphQL field, an MCP
/// operation and a WS message have no route data to hang one on, and each counts
/// against [`ThrottlerConfig`](crate::ThrottlerConfig)'s limit.
///
/// Binding scope chooses *which* units are measured, never what the guard
/// reads: `#[use_guards(ThrottlerGuard)]` measures one host or one operation,
/// `use_guards_global` measures everything the pool reaches — the units carrying
/// no `#[meta(Throttle)]` at the module default.
///
/// Injects the store as `Arc<dyn ThrottlerStore>`, so **one** guard serves
/// every backend: [`InMemoryThrottler`](crate::InMemoryThrottler) by default,
/// or a shared store (Redis) when its module is imported instead. The store
/// binding is what an app swaps — never the guard.
#[injectable]
pub struct ThrottlerGuard {
    #[inject]
    throttler: Arc<dyn ThrottlerStore>,
    /// The limit a route that pins no `#[meta(Throttle)]` runs under — the
    /// port's policy, resolved from `ThrottlerConfig` and registered by
    /// `ThrottlerModule::for_root`. It lives on the guard and not on the store
    /// because a store only counts; swapping the backend must move the counters
    /// and nothing else. **Injected, never defaulted**: a plain `Throttle` field
    /// would be filled with 60/minute by any path that builds the guard from
    /// the container — `providers = [ThrottlerGuard]` without `for_root` — and
    /// the configured limit would be replaced in silence. As a dependency, that
    /// composition fails the boot naming `Throttle` instead.
    #[inject]
    default: Arc<Throttle>,
}

impl ThrottlerGuard {
    /// Build the guard over a store, with the default limit for routes that pin
    /// none. `ThrottlerModule` uses it to register the guard as global
    /// infrastructure, so `#[use_guards(ThrottlerGuard)]` needs nothing in the
    /// controller module's `providers`.
    pub fn new(throttler: Arc<dyn ThrottlerStore>, default: Throttle) -> Self {
        Self {
            throttler,
            default: Arc::new(default),
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

        // Route-specific bucket. The window is per route (each route pins its
        // own `#[meta(Throttle)]`), so the counter must be per route too —
        // keying on IP alone lets every `ThrottlerGuard` route share one bucket,
        // so hammering a lenient route drains a strict route's budget.
        let ip = ClientId::from(ClientOrigin::of(req));
        // Prefer poem's matched-route *pattern* (`/users/:id`) so dynamic path
        // segments don't fragment the bucket. Fall back to the raw path when no
        // pattern was attached (e.g. a self-mounted endpoint) — correct for the
        // static brute-force case (`/login`), at the cost of fragmenting dynamic
        // paths into a bucket per concrete URL.
        let route = req
            .data::<PathPattern>()
            .map(|pattern| pattern.0.as_ref())
            .unwrap_or_else(|| req.uri().path());
        let key = bucket_key(&[&transport::HTTP, &route, &ip]);

        let decision = self.throttler.hit(&key, limit).await;
        if decision.allowed {
            return Ok(());
        }
        // Route and client as two fields, never the composite store key: an
        // operator filtering 429s by client address should not have to split a
        // value on U+001F, and "never hand-format columns" is the rule.
        tracing::warn!(
            target: crate::TARGET,
            transport = transport::HTTP,
            route = %route,
            client = %ip,
            retry_after = retry_after_secs(decision.retry_after),
            "rate limit exceeded",
        );
        Err(rate_limited(decision.retry_after))
    }

    /// GraphQL's unit is the **field**: one document may carry several, and the
    /// per-operation chain reaches this site once per field resolved. `/graphql`
    /// is `Exempt`, so nothing an HTTP-scope binding does reaches them.
    ///
    /// **The caller half is the actor, and it is read rather than invented.**
    /// The peer address is not reachable here — it lives on the poem `Request`,
    /// which no `GraphqlContextSeed` forwards — but the *principal* is: the
    /// operation runs inside the request scope the edge installed, so
    /// [`current_actor_id`](nest_rs_core::current_actor_id) answers for every
    /// authenticated caller.
    ///
    /// That distinction is the whole security property. Keyed on the field
    /// alone, every caller shares one bucket, and one client spending the
    /// window `429`s everybody — a limiter that is a denial-of-service
    /// amplifier rather than a defence. Keyed on the actor, a caller can only
    /// exhaust their own.
    ///
    /// An **anonymous** caller has no actor, and then the shared bucket is the
    /// honest answer rather than a chosen one — so it is reported, once per
    /// process, and only when it actually happens. The per-address half for
    /// that traffic is this same guard in `use_guards_global`, where `/graphql`
    /// is one HTTP request keyed on its client.
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

        let decision = self.throttler.hit(&key, *self.default).await;
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
    /// The caller half is the actor, read the same way and for the same reason
    /// as GraphQL's. The operation does run on a task rmcp spawned — but
    /// `nest_rs_mcp::propagate` installs the request scope *across* that spawn,
    /// so the correlation and its actor survive it. "The request is gone" was
    /// true of the peer address and not of the principal, and keying on neither
    /// made the limiter a shared kill switch.
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

        let decision = self.throttler.hit(&key, *self.default).await;
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

    /// WS's unit is the **event**, and the connection is the caller half this
    /// site really can see — no invention required, but no address either: a
    /// message is dispatched long after the upgrade's task-locals unwound, so
    /// the peer that keyed the `GET` is gone by then.
    ///
    /// A reconnect therefore opens a fresh bucket. That is bounded rather than
    /// open: the upgrade *is* an HTTP `GET`, so the same guard on the
    /// `#[gateway]` struct meters connection attempts per address, and this
    /// entry meters the traffic inside one connection — the flood the upgrade
    /// cannot see.
    #[cfg(feature = "ws")]
    async fn check_ws_message(
        &self,
        client: &WsClient,
        event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), Denial> {
        let connection = client.id();
        let key = bucket_key(&[&transport::WS, &event, &connection]);

        let decision = self.throttler.hit(&key, *self.default).await;
        if decision.allowed {
            return Ok(());
        }
        tracing::warn!(
            target: crate::TARGET,
            transport = transport::WS,
            event = %event,
            client = %connection,
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
/// single `#[query]` may bind it — the binding that once compiled and throttled
/// nothing.
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
///
/// Carried as a value rather than a `String` so the composite route+client key
/// is built in **one** allocation: the previous shape rendered the address to
/// its own `String` only to interpolate it into the real key a line later.
enum ClientId {
    /// The resolved client address.
    Ip(IpAddr),
    /// No address could be resolved — every caller shares one bucket. See
    /// [`warn_shared_bucket`].
    Shared,
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ip(ip) => write!(f, "{ip}"),
            Self::Shared => f.write_str("global"),
        }
    }
}

/// Turn the transport's [`ClientOrigin`] into a bucket key, warning on the two
/// resolutions that collapse every caller into one bucket.
///
/// The resolution itself — peer, trusted-proxy gate, rightmost non-trusted hop —
/// lives in `nest_rs_http::ClientOrigin`, so a `429` and the log line explaining
/// it always name the same caller.
impl From<ClientOrigin> for ClientId {
    fn from(origin: ClientOrigin) -> Self {
        match origin {
            ClientOrigin::Peer(ip) | ClientOrigin::Forwarded(ip) => Self::Ip(ip),
            // The peer is a trusted proxy that forwarded no client address: it
            // is still an address, but everyone behind it shares this bucket.
            ClientOrigin::TrustedProxy(ip) => {
                static SEEN: AtomicBool = AtomicBool::new(false);
                warn_shared_bucket(
                    &SEEN,
                    "trusted_proxy_without_forwarded_for",
                    "the direct peer is a trusted proxy but sent no usable X-Forwarded-For — \
                     every caller behind it shares one rate-limit bucket; make the proxy forward \
                     the client address",
                );
                Self::Ip(ip)
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
/// reported fallback when there is none.
///
/// **This is the difference between a rate limiter and a denial-of-service
/// amplifier.** Keyed on the operation alone, every caller shares one bucket and
/// one client spending the window `429`s everybody. The peer address is not
/// reachable at an in-band site — it lives on the poem `Request` — but the
/// *actor* is: `nest_rs_mcp::propagate` and the GraphQL edge both install the
/// request scope around the operation, so `current_actor_id()` answers for
/// every authenticated caller.
///
/// An anonymous caller has none, and then the shared bucket is the honest answer
/// rather than a chosen one — reported once per process, and only when it
/// actually happens. The per-address half for that traffic is the same guard in
/// `use_guards_global`, where the carrying HTTP request is keyed on its client.
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

/// What an unauthenticated caller is keyed as.
///
/// A named value rather than an empty string: `""` would be indistinguishable
/// from an actor named that, which is the sentinel `current_actor_id` refuses to
/// return for the same reason.
#[cfg(any(feature = "graphql", feature = "mcp"))]
const ANONYMOUS_CALLER: &str = "<anonymous>";

/// Report a keying degradation **once per process**, at `warn`.
///
/// Both degradations are misconfigurations that stay invisible until an outage:
/// the throttler keeps answering, it just stops distinguishing callers. They are
/// a structural fact of the deployment, not a per-request event, so this dedups
/// rather than spamming a line per request.
///
/// The dedup is one flag per reason, supplied by the call site, rather than a
/// set of reasons behind a process-wide `Mutex`. The reasons are a closed set of
/// four `&'static str`s that never grows at runtime, so the set could only ever
/// answer what a `bool` answers — while the lock sat on the anonymous in-band
/// path, which is every request on an unauthenticated GraphQL or MCP surface.
/// Serializing all of them through one mutex is a ceiling on exactly the traffic
/// a rate limiter exists to survive.
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
    use std::net::IpAddr;

    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test literal is an IP")
    }

    // The resolution itself is `nest_rs_http::ClientOrigin`'s contract and is
    // tested there — including every `X-Forwarded-For` spoofing case (B-HTTP-1).
    // What belongs here is the mapping onto a bucket: which origins get their
    // own bucket, and which collapse everyone into one.
    #[test]
    fn every_resolved_address_gets_its_own_bucket() {
        let addr = ip("203.0.113.50");
        assert_eq!(
            ClientId::from(ClientOrigin::Peer(addr)).to_string(),
            "203.0.113.50"
        );
        assert_eq!(
            ClientId::from(ClientOrigin::Forwarded(addr)).to_string(),
            "203.0.113.50",
            "a hop a trusted proxy forwarded keys the same as a direct peer",
        );
    }

    // A proxy that forwards nothing still has an address, so the bucket is
    // named — but everyone behind it shares it, which is why it warns.
    #[test]
    fn a_trusted_proxy_that_forwards_nothing_keys_on_the_proxy() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            ClientId::from(ClientOrigin::TrustedProxy(proxy)).to_string(),
            "10.0.0.1",
        );
    }

    // No peer at all (a unix socket, a test stub): one shared bucket. Named
    // `global` rather than empty so it is legible in metrics.
    #[test]
    fn no_peer_address_falls_back_to_a_named_global_bucket() {
        assert_eq!(ClientId::from(ClientOrigin::Unknown).to_string(), "global");
    }

    /// Both degradations keep the throttler *answering* — it just stops
    /// distinguishing callers, so one client can exhaust everyone's budget and
    /// no status code changes. They are invisible until the outage, which is
    /// why they are `warn` and why the line carries the remedy.
    ///
    /// Deduped once per process by reason. That is safe to assert on because
    /// nextest runs each test in its own process — a sibling burning the same
    /// reason cannot silence this one, and `cargo test`'s shared binary is
    /// unsupported here anyway (`CLAUDE.md`: the runner is nextest).
    #[test]
    fn a_degraded_keying_is_reported_once_with_its_remedy() {
        let logs = nest_rs_testing::LogCapture::install();
        // A unix socket or a proxy that hides the peer: no address at all, so
        // every caller lands in one bucket.
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

        // And the second call is silent: a per-request line for a structural
        // fact would bury the events an incident actually queries.
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
