//! [`ThrottlerGuard`] — per-route rate-limiting guard.

use std::net::IpAddr;
use std::sync::Arc;

use nest_rs_core::{HandlerMetadata, Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{Reflector, async_trait};
use poem::{PathPattern, Request};

use crate::rate::Throttle;
use crate::store::ThrottlerStore;

/// Bind per route with `#[use_guards(ThrottlerGuard)]`. Reads the route's
/// `#[meta(Throttle::...)]` via the [`Reflector`], falling back to the module
/// default; rejects with `429` + `Retry-After`.
///
/// Must be a per-route guard, not a global one: a global guard runs before
/// routing, so the route's `#[meta(Throttle)]` is not yet attached.
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
        let ip = client_key(req, self.throttler.trusted_proxies());
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
            target: "nest_rs::throttler",
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

/// The identity a rate-limit bucket is keyed on.
///
/// Carried as a value rather than a `String` so the composite route+client key
/// is built in **one** allocation: the previous shape rendered the address to
/// its own `String` only to interpolate it into the real key a line later.
enum ClientId {
    /// The resolved client address.
    Ip(IpAddr),
    /// No address could be resolved — every caller shares one bucket. See the
    /// `warn` in [`client_key_from`].
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

/// Direct peer IP unless the peer is a configured trusted proxy — then the
/// **rightmost** `X-Forwarded-For` hop that is not itself a trusted proxy.
fn client_key(req: &Request, trusted_proxies: &[IpAddr]) -> ClientId {
    let peer = req.remote_addr().as_socket_addr().map(|addr| addr.ip());
    client_key_from(
        req.headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
        peer,
        trusted_proxies,
    )
}

/// Resolve the rate-limit identity from the direct peer and the (untrusted)
/// `X-Forwarded-For` chain.
///
/// `X-Forwarded-For` is only consulted when the direct peer is a configured
/// trusted proxy — otherwise a client sets it freely. A proxy **appends** the
/// address it received the request from to the *right* of the chain, so the
/// genuine client is the rightmost hop that is not itself a trusted proxy: walk
/// right-to-left, skip trusted hops, take the first non-trusted one. A client
/// can only **prepend** spoofed entries (they land to the left of the genuine
/// hop), so this keying can't be rotated to mint fresh buckets or forged to
/// target a victim's bucket (B-HTTP-1).
fn client_key_from(
    forwarded_for: Option<&str>,
    peer: Option<IpAddr>,
    trusted_proxies: &[IpAddr],
) -> ClientId {
    let Some(peer) = peer else {
        warn_shared_bucket(
            "no_peer_address",
            "no peer address (unix socket, or a proxy that hides it) — every caller shares one \
             rate-limit bucket, so a single client can exhaust the budget for all of them",
        );
        return ClientId::Shared;
    };
    // Anyone but a trusted proxy could forge X-Forwarded-For, so ignore it.
    if !trusted_proxies.contains(&peer) {
        return ClientId::Ip(peer);
    }
    if let Some(chain) = forwarded_for {
        let hops: Vec<IpAddr> = chain
            .split(',')
            .map(str::trim)
            .filter(|hop| !hop.is_empty())
            .filter_map(|hop| hop.parse::<IpAddr>().ok())
            .collect();
        // Rightmost hop appended by infrastructure that isn't itself trusted.
        if let Some(client) = hops.iter().rev().find(|ip| !trusted_proxies.contains(ip)) {
            return ClientId::Ip(*client);
        }
        // Every recorded hop is itself a trusted proxy: fall back to the
        // leftmost (outermost) recorded address rather than to nothing.
        if let Some(outermost) = hops.first() {
            return ClientId::Ip(*outermost);
        }
    }
    // No usable forwarded hop (missing / empty / all-unparseable): the trusted
    // proxy itself is the client — which means everyone behind it shares a
    // bucket, the same failure mode as having no peer address at all.
    warn_shared_bucket(
        "trusted_proxy_without_forwarded_for",
        "the direct peer is a trusted proxy but sent no usable X-Forwarded-For — every caller \
         behind it shares one rate-limit bucket; make the proxy forward the client address",
    );
    ClientId::Ip(peer)
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
            target: "nest_rs::throttler",
            reason,
            detail,
            "rate-limit keying degraded to a shared bucket",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::client_key_from;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn direct_clients_ignore_spoofed_x_forwarded_for() {
        let key = client_key_from(
            Some("203.0.113.50"),
            Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))),
            &[],
        );
        assert_eq!(key.to_string(), "192.0.2.10");
    }

    // B-HTTP-1: behind a single trusted proxy the real client is the hop the
    // proxy APPENDED (the rightmost non-trusted), not the leftmost — the
    // leftmost is the client-authored, spoofable value.
    #[test]
    fn trusted_proxy_uses_the_rightmost_untrusted_hop_as_the_client() {
        let proxy = ip("10.0.0.1");
        // The proxy received the request from 192.0.2.1 and appended it; the
        // leftmost "203.0.113.50" is a header the client set.
        let key = client_key_from(Some("203.0.113.50, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(key.to_string(), "192.0.2.1");
    }

    // B-HTTP-1 (the core exploit): an attacker prepends a random/victim IP to
    // X-Forwarded-For. The genuine hop the trusted proxy appends sits to its
    // right and is the one selected — the prepended value can neither mint a
    // fresh bucket nor drain a victim's.
    #[test]
    fn a_prepended_spoofed_hop_cannot_change_the_key() {
        let proxy = ip("10.0.0.1");
        let real = client_key_from(Some("203.0.113.50"), Some(proxy), &[proxy]);
        // Attacker rotates a random leading hop: the key is unchanged.
        let spoofed = client_key_from(Some("1.2.3.4, 203.0.113.50"), Some(proxy), &[proxy]);
        assert_eq!(real.to_string(), "203.0.113.50");
        assert_eq!(spoofed.to_string(), "203.0.113.50");
        // Attacker forges a victim's IP as the leading hop: still unchanged.
        let targeted = client_key_from(Some("198.51.100.7, 203.0.113.50"), Some(proxy), &[proxy]);
        assert_eq!(targeted.to_string(), "203.0.113.50");
    }

    // A two-layer proxy chain (LB → nginx → app): both infra hops are trusted,
    // so the client is the rightmost hop that is NOT a trusted proxy.
    #[test]
    fn two_layer_proxy_chain_selects_the_real_client() {
        let nginx = ip("10.0.0.1");
        let lb = ip("10.0.0.2");
        // client(203.0.113.50) → lb appended it → nginx appended lb.
        let key = client_key_from(Some("203.0.113.50, 10.0.0.2"), Some(nginx), &[nginx, lb]);
        assert_eq!(key.to_string(), "203.0.113.50");
    }

    // Spoof inside a two-layer chain: the attacker prepends a hop, but every
    // trusted infra hop is still skipped from the right and the first
    // non-trusted hop below them is the genuine client.
    #[test]
    fn a_spoofed_hop_in_a_two_layer_chain_is_ignored() {
        let nginx = ip("10.0.0.1");
        let lb = ip("10.0.0.2");
        let key = client_key_from(
            Some("9.9.9.9, 203.0.113.50, 10.0.0.2"),
            Some(nginx),
            &[nginx, lb],
        );
        assert_eq!(key.to_string(), "203.0.113.50");
    }

    // Degenerate: every recorded hop is itself a trusted proxy — fall back to
    // the leftmost (outermost) recorded address rather than to nothing.
    #[test]
    fn all_trusted_hops_fall_back_to_the_outermost() {
        let a = ip("10.0.0.1");
        let b = ip("10.0.0.2");
        let c = ip("10.0.0.3");
        let key = client_key_from(Some("10.0.0.2, 10.0.0.3"), Some(a), &[a, b, c]);
        assert_eq!(key.to_string(), "10.0.0.2");
    }

    // When the peer is unknown (no socket addr — e.g. a UDS connection or a
    // test stub), fall back to a single shared bucket. A bug that returned
    // `""` here would create one nameless bucket where everyone collides
    // anyway, but `"global"` is louder in metrics.
    #[test]
    fn no_peer_addr_falls_back_to_a_named_global_bucket() {
        let key = client_key_from(Some("203.0.113.50"), None, &[]);
        assert_eq!(key.to_string(), "global");
        let key = client_key_from(None, None, &[]);
        assert_eq!(key.to_string(), "global");
    }

    // Trusted proxy + no X-Forwarded-For header: the trusted proxy itself
    // gets rate-limited as the client. Different from the "spoofed" case
    // because the proxy isn't claiming a real-client IP.
    #[test]
    fn trusted_proxy_without_x_forwarded_for_is_the_client() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(None, Some(proxy), &[proxy]);
        assert_eq!(key.to_string(), "10.0.0.1");
    }

    // Trusted proxy + empty X-Forwarded-For: no usable hop ⇒ fall back to the
    // proxy IP, not "".
    #[test]
    fn trusted_proxy_with_empty_forwarded_chain_falls_back_to_proxy_ip() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some(""), Some(proxy), &[proxy]);
        assert_eq!(key.to_string(), "10.0.0.1");
        let key = client_key_from(Some(",,,"), Some(proxy), &[proxy]);
        assert_eq!(key.to_string(), "10.0.0.1");
    }

    // Trusted proxy + an unparseable prepended hop: it is skipped, and the
    // valid appended hop is the client. Prevents accepting a user-supplied
    // string as a client key.
    #[test]
    fn trusted_proxy_skips_an_unparseable_hop() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some("not-an-ip, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(key.to_string(), "192.0.2.1");
    }

    // Trusted proxy + IPv6 client. The parse accepts any `IpAddr` shape; this
    // pins that v6 chains are handled too (a future regression that limited the
    // parse to v4 would fail here).
    #[test]
    fn trusted_proxy_with_ipv6_client_is_accepted() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some("2001:db8::1"), Some(proxy), &[proxy]);
        assert_eq!(key.to_string(), "2001:db8::1");
    }

    // Peer ≠ trusted proxy ⇒ the X-Forwarded-For is ignored even when
    // structurally valid. The spoof case at the top covers a header set; this
    // pins a multi-entry header from an untrusted peer.
    #[test]
    fn untrusted_peer_ignores_a_well_formed_forwarded_chain() {
        let key = client_key_from(
            Some("203.0.113.50, 10.0.0.99"),
            Some(ip("192.0.2.10")),
            &[ip("10.0.0.1")],
        );
        assert_eq!(key.to_string(), "192.0.2.10");
    }

    // Surrounding whitespace around the selected hop is trimmed — RFC 7239
    // allows it and real proxies emit it.
    #[test]
    fn trusted_proxy_trims_whitespace_around_the_selected_hop() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some("   203.0.113.50  "), Some(proxy), &[proxy]);
        assert_eq!(key.to_string(), "203.0.113.50");
    }
}
