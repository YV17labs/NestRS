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

        let decision = self.throttler.hit(&key, limit);
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

/// Direct peer IP unless the peer is a configured trusted proxy — then the
/// leftmost `X-Forwarded-For` hop.
fn client_key(req: &Request, trusted_proxies: &[IpAddr]) -> String {
    let peer = req.remote_addr().as_socket_addr().map(|addr| addr.ip());
    client_key_from(
        req.headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
        peer,
        trusted_proxies,
    )
}

fn client_key_from(
    forwarded_for: Option<&str>,
    peer: Option<IpAddr>,
    trusted_proxies: &[IpAddr],
) -> String {
    if let Some(peer) = peer {
        if trusted_proxies.contains(&peer)
            && let Some(client) = forwarded_for
                .and_then(|chain| chain.split(',').next())
                .map(str::trim)
                .filter(|ip| !ip.is_empty() && ip.parse::<IpAddr>().is_ok())
        {
            return client.to_owned();
        }
        return peer.to_string();
    }
    "global".to_owned()
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
        assert_eq!(key, "192.0.2.10");
    }

    #[test]
    fn trusted_proxies_use_the_leftmost_forwarded_client() {
        let proxy = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let key = client_key_from(Some("203.0.113.50, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(key, "203.0.113.50");
    }

    // When the peer is unknown (no socket addr — e.g. a UDS connection or a
    // test stub), fall back to a single shared bucket. A bug that returned
    // `""` here would create one nameless bucket where everyone collides
    // anyway, but `"global"` is louder in metrics.
    #[test]
    fn no_peer_addr_falls_back_to_a_named_global_bucket() {
        let key = client_key_from(Some("203.0.113.50"), None, &[]);
        assert_eq!(key, "global");
        let key = client_key_from(None, None, &[]);
        assert_eq!(key, "global");
    }

    // Trusted proxy + no X-Forwarded-For header: the trusted proxy itself
    // gets rate-limited as the client. Different from the "spoofed" case
    // because the proxy isn't claiming a real-client IP.
    #[test]
    fn trusted_proxy_without_x_forwarded_for_is_the_client() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(None, Some(proxy), &[proxy]);
        assert_eq!(key, "10.0.0.1");
    }

    // Trusted proxy + empty X-Forwarded-For: an empty hop is rejected as an
    // un-spoofable client claim — fall back to the proxy IP, not "".
    #[test]
    fn trusted_proxy_with_empty_forwarded_chain_falls_back_to_proxy_ip() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some(""), Some(proxy), &[proxy]);
        assert_eq!(key, "10.0.0.1");
        let key = client_key_from(Some(",,,"), Some(proxy), &[proxy]);
        // First hop trims to "" — invalid IP — fall back.
        assert_eq!(key, "10.0.0.1");
    }

    // Trusted proxy + unparseable first hop ⇒ fall back. Important: prevents
    // accepting "user-supplied-string" as a client key.
    #[test]
    fn trusted_proxy_with_unparseable_first_hop_falls_back_to_proxy_ip() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some("not-an-ip, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(key, "10.0.0.1");
    }

    // Trusted proxy + IPv6 first hop. The parse accepts any `IpAddr` shape;
    // this pins that v6 chains are handled too (a future regression that
    // limited the parse to v4 would fail here).
    #[test]
    fn trusted_proxy_with_ipv6_first_hop_is_accepted() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some("2001:db8::1, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(key, "2001:db8::1");
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
        assert_eq!(key, "192.0.2.10");
    }

    // Surrounding whitespace in the first hop is trimmed — RFC 7239 allows
    // them and real proxies emit them.
    #[test]
    fn trusted_proxy_trims_whitespace_around_the_first_hop() {
        let proxy = ip("10.0.0.1");
        let key = client_key_from(Some("   203.0.113.50  , 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(key, "203.0.113.50");
    }
}
