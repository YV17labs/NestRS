//! [`ThrottlerGuard`] — rate-limiting guard.

use std::net::IpAddr;
use std::sync::Arc;

use nest_rs_core::{HandlerMetadata, Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{ClientOrigin, Reflector, async_trait};
use poem::{PathPattern, Request};

use crate::rate::Throttle;
use crate::store::ThrottlerStore;

/// Reads the route's `#[meta(Throttle::...)]` via the [`Reflector`], falling
/// back to the module default; rejects with `429` + `Retry-After`.
///
/// Binding scope chooses *which* routes are measured, never what the guard
/// reads: `#[use_guards(ThrottlerGuard)]` measures one controller or route,
/// `use_guards_global` measures every route the pool reaches — the ones
/// carrying no `#[meta(Throttle)]` at the module default.
///
/// Injects the store as `Arc<dyn ThrottlerStore>`, so **one** guard serves
/// every backend: [`InMemoryThrottler`](crate::InMemoryThrottler) by default,
/// or a shared store (Redis) when its module is imported instead. The store
/// binding is what an app swaps — never the guard.
#[injectable]
pub struct ThrottlerGuard {
    #[inject]
    throttler: Arc<dyn ThrottlerStore>,
}

impl ThrottlerGuard {
    /// Build the guard over a store. `ThrottlerModule` uses it to register the
    /// guard as global infrastructure, so `#[use_guards(ThrottlerGuard)]` needs
    /// nothing in the controller module's `providers`.
    pub fn new(throttler: Arc<dyn ThrottlerStore>) -> Self {
        Self { throttler }
    }
}

impl Layer for ThrottlerGuard {}

#[async_trait]
impl Guard for ThrottlerGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let limit = Reflector::new(req)
            .get::<Throttle>()
            .copied()
            .unwrap_or_else(|| self.throttler.default_limit());

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
        // U+001F (unit separator) can appear in neither a route pattern nor an
        // IP, so the composite key never collides across the join.
        let key = format!("{route}\u{1f}{ip}");

        let decision = self.throttler.hit(&key, limit).await;
        if decision.allowed {
            return Ok(());
        }
        tracing::warn!(
            target: crate::TARGET,
            key = %key,
            retry_after = decision.retry_after.as_secs(),
            "rate limit exceeded",
        );
        Err(Denial::rate_limited(
            decision.retry_after.as_secs() as u32,
            "Too Many Requests",
        ))
    }
}

/// HTTP, and only HTTP: the bucket is keyed on the matched route pattern, which
/// no other edge has. Binding this beside a `#[query]` or a
/// `#[subscribe_message]` is the mistake the markers exist to refuse — it is the
/// original one, and it throttled nothing.
impl nest_rs_guards::HttpGuard for ThrottlerGuard {}

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
                warn_shared_bucket(
                    "trusted_proxy_without_forwarded_for",
                    "the direct peer is a trusted proxy but sent no usable X-Forwarded-For — \
                     every caller behind it shares one rate-limit bucket; make the proxy forward \
                     the client address",
                );
                Self::Ip(ip)
            }
            ClientOrigin::Unknown => {
                warn_shared_bucket(
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

/// Report a keying degradation **once per process**, at `warn`.
///
/// Both degradations are misconfigurations that stay invisible until an outage:
/// the throttler keeps answering, it just stops distinguishing callers. They are
/// a structural fact of the deployment, not a per-request event, so this dedups
/// by reason rather than spamming a line per request.
fn warn_shared_bucket(reason: &'static str, detail: &'static str) {
    use std::collections::HashSet;
    use std::sync::{LazyLock, Mutex};

    static SEEN: LazyLock<Mutex<HashSet<&'static str>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));

    // On a poisoned lock, emit: a duplicate diagnostic is harmless, a swallowed
    // one hides the misconfiguration this exists to surface.
    let first_time = SEEN
        .lock()
        .map(|mut seen| seen.insert(reason))
        .unwrap_or(true);
    if first_time {
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
            "nest_rs::throttler",
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
                "nest_rs::throttler",
                "rate-limit keying degraded to a shared bucket",
            )
            .len(),
            1,
            "reported once per process, per reason",
        );
    }
}
