//! [`ThrottlerGuard`] — per-route rate-limiting guard.

use std::net::IpAddr;
use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_http::{Guard, Reflector, async_trait};
use poem::http::{StatusCode, header};
use poem::{Request, Response};

use crate::store::InMemoryThrottler;
use crate::throttle::Throttle;

/// Bind per route with `#[use_guards(ThrottlerGuard)]`. Reads the route's
/// `#[meta(Throttle::...)]` via the [`Reflector`], falling back to the module
/// default; rejects with `429` + `Retry-After`.
///
/// Must be a per-route guard, not a global one: a global guard runs before
/// routing, so the route's `#[meta(Throttle)]` is not yet attached.
#[injectable]
pub struct ThrottlerGuard {
    #[inject]
    throttler: Arc<InMemoryThrottler>,
}

#[async_trait]
impl Guard for ThrottlerGuard {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        let limit = Reflector::new(req)
            .get::<Throttle>()
            .copied()
            .unwrap_or_else(|| self.throttler.default_limit());

        let decision = self
            .throttler
            .hit(&client_key(req, self.throttler.trusted_proxies()), limit);
        if decision.allowed {
            return Ok(());
        }
        Err(Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(
                header::RETRY_AFTER,
                decision.retry_after.as_secs().to_string(),
            )
            .body("Too Many Requests"))
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
}
