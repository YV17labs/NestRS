//! Client IP extractor with a documented fallback chain.
//!
//! Resolution order, first hit wins:
//!
//! 1. the transport peer reported by poem (`req.remote_addr().as_socket_addr()`);
//! 2. the leftmost entry of the `X-Forwarded-For` header (parsed as `IpAddr`,
//!    optionally with a port, including the bracketed IPv6 forms `[ip]` and
//!    `[ip]:port` per RFC 7239);
//! 3. the `X-Real-IP` header (parsed as `IpAddr`);
//! 4. `0.0.0.0` as a last-resort default — the extractor never fails.
//!
//! [`ClientIp::forwarded`] is `true` when the address came from one of the
//! headers (2 or 3), `false` when it came from the peer socket (1) or the
//! default (4).
//!
//! # Security
//!
//! **`X-Forwarded-For` and `X-Real-IP` are spoofable.** Any client can put
//! whatever value it likes in those headers; the extractor parses them as a
//! best-effort fallback only. If your deployment terminates TLS at a load
//! balancer, the load balancer must **strip and rewrite** these headers so
//! only its own value reaches the app — otherwise a caller can forge the
//! client IP at will. Trusted-proxy validation (peer ∈ allow-list before
//! honoring `X-Forwarded-For`) is the throttler's job, not this extractor's.
//!
//! Treat the result as observational (logging, geolocation hints, sampling
//! keys), never as an authentication or authorization input.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use poem::{FromRequest, Request, RequestBody, Result};

/// Best-effort client IP for the current request. Always present (see the
/// module doc for the resolution order and the security caveat).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ClientIp {
    /// The resolved address. `0.0.0.0` only when nothing in the chain matched.
    pub ip: IpAddr,
    /// `true` when the address came from `X-Forwarded-For` or `X-Real-IP`,
    /// `false` when it came from the transport peer or the default.
    pub forwarded: bool,
}

impl ClientIp {
    /// Last-resort default: `0.0.0.0`, `forwarded = false`.
    pub const fn unknown() -> Self {
        Self {
            ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            forwarded: false,
        }
    }
}

/// Parse one `X-Forwarded-For` entry — strip whitespace, accept a bare IP,
/// `IP:port`, or a bracketed IPv6 (`[ip]` or `[ip]:port` per RFC 7239 — nginx
/// and HAProxy emit the bracketed form).
fn parse_forwarded_entry(raw: &str) -> Option<IpAddr> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
        && let Ok(ip) = inner.parse::<IpAddr>()
    {
        return Some(ip);
    }
    trimmed.parse::<SocketAddr>().ok().map(|sa| sa.ip())
}

fn resolve(req: &Request) -> ClientIp {
    if let Some(addr) = req.remote_addr().as_socket_addr() {
        return ClientIp {
            ip: addr.ip(),
            forwarded: false,
        };
    }

    if let Some(ip) = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|chain| chain.split(',').next())
        .and_then(parse_forwarded_entry)
    {
        return ClientIp {
            ip,
            forwarded: true,
        };
    }

    if let Some(ip) = req
        .headers()
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.trim().parse::<IpAddr>().ok())
    {
        return ClientIp {
            ip,
            forwarded: true,
        };
    }

    ClientIp::unknown()
}

impl<'a> FromRequest<'a> for ClientIp {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        Ok(resolve(req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req_with_header(name: &'static str, value: &'static str) -> Request {
        Request::builder().header(name, value).finish()
    }

    #[tokio::test]
    async fn missing_everything_falls_back_to_0_0_0_0_not_forwarded() {
        let req = Request::builder().finish();
        let (req, mut body) = req.split();
        let ip = ClientIp::from_request(&req, &mut body)
            .await
            .expect("infallible");
        assert_eq!(ip, ClientIp::unknown());
        assert!(!ip.forwarded);
    }

    #[tokio::test]
    async fn xff_leftmost_entry_wins_over_xri() {
        // A built `Request` has no socket peer, so headers take over.
        let req = Request::builder()
            .header("x-forwarded-for", "203.0.113.10, 198.51.100.10")
            .header("x-real-ip", "198.51.100.20")
            .finish();
        let (req, mut body) = req.split();
        let ip = ClientIp::from_request(&req, &mut body).await.unwrap();
        assert_eq!(ip.ip, IpAddr::from([203, 0, 113, 10]));
        assert!(ip.forwarded);
    }

    #[tokio::test]
    async fn xff_with_whitespace_around_the_leftmost_entry_is_trimmed() {
        let req = req_with_header("x-forwarded-for", "   203.0.113.7  , 10.0.0.1");
        let (req, mut body) = req.split();
        let ip = ClientIp::from_request(&req, &mut body).await.unwrap();
        assert_eq!(ip.ip, IpAddr::from([203, 0, 113, 7]));
        assert!(ip.forwarded);
    }

    #[tokio::test]
    async fn xff_with_port_on_the_leftmost_entry_keeps_only_the_ip() {
        let req = req_with_header("x-forwarded-for", "203.0.113.42:51000, 10.0.0.1");
        let (req, mut body) = req.split();
        let ip = ClientIp::from_request(&req, &mut body).await.unwrap();
        assert_eq!(ip.ip, IpAddr::from([203, 0, 113, 42]));
        assert!(ip.forwarded);
    }

    #[tokio::test]
    async fn x_real_ip_used_when_xff_is_absent() {
        let req = req_with_header("x-real-ip", "198.51.100.20");
        let (req, mut body) = req.split();
        let ip = ClientIp::from_request(&req, &mut body).await.unwrap();
        assert_eq!(ip.ip, IpAddr::from([198, 51, 100, 20]));
        assert!(ip.forwarded);
    }

    #[tokio::test]
    async fn malformed_xff_leftmost_entry_falls_through_to_x_real_ip() {
        // An empty leading entry (`, …`) is not a valid IP — XRI must win.
        let req = Request::builder()
            .header("x-forwarded-for", "not-an-ip, 10.0.0.1")
            .header("x-real-ip", "198.51.100.20")
            .finish();
        let (req, mut body) = req.split();
        let ip = ClientIp::from_request(&req, &mut body).await.unwrap();
        assert_eq!(ip.ip, IpAddr::from([198, 51, 100, 20]));
        assert!(ip.forwarded);
    }

    #[tokio::test]
    async fn malformed_everything_falls_through_to_the_default() {
        let req = Request::builder()
            .header("x-forwarded-for", "garbage")
            .header("x-real-ip", "also-garbage")
            .finish();
        let (req, mut body) = req.split();
        let ip = ClientIp::from_request(&req, &mut body).await.unwrap();
        assert_eq!(ip, ClientIp::unknown());
    }

    #[test]
    fn parse_forwarded_entry_accepts_ipv4_ipv6_and_socketaddr() {
        let ipv6: IpAddr = "2001:db8::1".parse().unwrap();

        // Bare IPv4.
        assert_eq!(
            parse_forwarded_entry("  203.0.113.1  "),
            Some(IpAddr::from([203, 0, 113, 1])),
        );
        // Bare IPv4 again, no padding.
        assert_eq!(
            parse_forwarded_entry("10.0.0.1"),
            Some(IpAddr::from([10, 0, 0, 1])),
        );
        // IPv4 with port (SocketAddr path).
        assert_eq!(
            parse_forwarded_entry("10.0.0.1:8080"),
            Some(IpAddr::from([10, 0, 0, 1])),
        );
        // Bare IPv6.
        assert_eq!(parse_forwarded_entry("2001:db8::1"), Some(ipv6));
        // Bracketed IPv6 with port (regression pin).
        assert_eq!(parse_forwarded_entry("[2001:db8::1]:8080"), Some(ipv6));
        // Bracketed IPv6 without a port — the case nginx/HAProxy emit per
        // RFC 7239 and that the bare-IpAddr / SocketAddr parsers both reject.
        assert_eq!(parse_forwarded_entry("[2001:db8::1]"), Some(ipv6));
        // Malformed bracketed entry fails gracefully.
        assert_eq!(parse_forwarded_entry("[malformed::]"), None);
        assert_eq!(parse_forwarded_entry(""), None);
        assert_eq!(parse_forwarded_entry("not-an-ip"), None);
    }
}
