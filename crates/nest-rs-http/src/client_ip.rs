//! Who the request came from — [`ClientOrigin`], the one resolution every
//! consumer shares, and [`ClientIp`], the extractor over it.
//!
//! # Why a trusted-proxy list is not optional
//!
//! `X-Forwarded-For` and `X-Real-IP` are client-authored strings. Honoring them
//! unconditionally lets any caller claim any address; ignoring them entirely
//! means an app behind a load balancer only ever sees the balancer. Neither is
//! usable, so the resolution is gated on the **direct peer**:
//!
//! 1. no peer address at all (unix socket, or a proxy that hides it) ⇒
//!    [`ClientOrigin::Unknown`];
//! 2. the peer is not in `NESTRS_HTTP__TRUSTED_PROXIES` ⇒ that peer *is* the
//!    client ([`ClientOrigin::Peer`]) and the headers are ignored;
//! 3. the peer is a trusted proxy ⇒ the forwarding headers are read, and the
//!    client is the **rightmost** `X-Forwarded-For` hop that is not itself a
//!    trusted proxy ([`ClientOrigin::Forwarded`]).
//!
//! **Rightmost, never leftmost.** A proxy *appends* the address it received the
//! request from to the right of the chain, so the genuine client is the last
//! hop infrastructure wrote. A caller can only *prepend*, and a prepended entry
//! lands to the left of the genuine one — which is why it can neither mint a
//! fresh identity nor impersonate a victim's (B-HTTP-1). Keying on the leftmost
//! hop is the spoofable rule.
//!
//! With no trusted proxy configured — the default — step 2 always wins and the
//! headers are never read. That is the safe default, not a limitation: an app
//! that is genuinely behind a balancer names it, and only then does the
//! framework believe what the balancer says.
//!
//! # Two consumers, one answer
//!
//! [`ClientIp`] (observational: logging, geolocation hints, sampling keys) and
//! the throttler's rate-limit bucket must not disagree about who the caller is
//! — a request rate-limited as one address and logged as another is
//! unauditable. Both go through [`ClientOrigin::of`], so the deployment
//! declares its proxies once, in `HttpConfig`.
//!
//! Treat the result as observational, never as an authentication or
//! authorization input: the peer is trustworthy, the hop behind it is only as
//! trustworthy as the proxy that wrote it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use nest_rs_core::current_request_scope;
use poem::{FromRequest, Request, RequestBody, Result};

use crate::HttpConfig;

/// Who a request is attributed to, and on what evidence. The variants are what
/// let each consumer react differently to the same resolution: the throttler
/// warns on [`Unknown`](Self::Unknown) and
/// [`TrustedProxy`](Self::TrustedProxy) (both collapse every caller into one
/// bucket), while [`ClientIp`] only needs the address and whether a header
/// supplied it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ClientOrigin {
    /// The direct transport peer, which is not a configured trusted proxy.
    /// Forwarding headers were present or not — either way they were ignored.
    Peer(IpAddr),
    /// A hop read from `X-Forwarded-For` / `X-Real-IP`, admitted because the
    /// direct peer is a configured trusted proxy.
    Forwarded(IpAddr),
    /// The peer is a trusted proxy but forwarded no usable client address.
    /// Every caller behind it is indistinguishable.
    TrustedProxy(IpAddr),
    /// No peer address at all — a unix socket, or a proxy that hides it.
    Unknown,
}

impl ClientOrigin {
    /// Resolve from a live request, reading the trusted-proxy list off the
    /// [`HttpConfig`] the app booted with.
    ///
    /// The list is a **boot-time constant**, so it is read from the container
    /// rather than pushed through per-request state: an extension insert costs
    /// one box each and the first one allocates the whole per-request anymap,
    /// which is the trade
    /// [`RequestScope`](nest_rs_core::RequestScope)'s own doc rejects. Off the
    /// request task (a hand-built `Request` in a unit test) nothing is trusted,
    /// which is the same answer an unconfigured deployment gives.
    pub fn of(req: &Request) -> Self {
        let scope = current_request_scope();
        let config = scope.as_ref().and_then(|s| s.root().get::<HttpConfig>());
        Self::of_with(
            req,
            config.as_deref().map_or(&[][..], |c| &c.trusted_proxies),
        )
    }

    /// [`of`](Self::of) against an explicit list — what the transport edge
    /// needs, because it runs *before* the request scope that `of` reads the
    /// list off exists.
    ///
    /// Both entry points funnel here so the forwarding header names and the
    /// argument order are written once: a resolution spelled twice is two
    /// answers to "who called" the day one of them learns about `Forwarded:`.
    pub(crate) fn of_with(req: &Request, trusted_proxies: &[IpAddr]) -> Self {
        // With nothing declared trusted — the default deployment — no
        // forwarding header can be believed, so none is looked up. `resolve`
        // reaches the same answer from the peer alone; skipping the reads only
        // spares it two header probes and two UTF-8 validations per request,
        // and leaves the decision itself in one place.
        let header = |name: &str| {
            (!trusted_proxies.is_empty())
                .then(|| req.headers().get(name)?.to_str().ok())
                .flatten()
        };
        Self::resolve(
            header("x-forwarded-for"),
            header("x-real-ip"),
            req.remote_addr().as_socket_addr().map(SocketAddr::ip),
            trusted_proxies,
        )
    }

    /// Whether the direct peer is declared infrastructure — the one fact that
    /// decides whether *any* header this caller sent may be believed.
    ///
    /// Read by the correlation id's gate on `X-Request-Id`, so the deployment
    /// cannot end up believing a header for the client's address while
    /// disbelieving it for the id, or the reverse. `Peer` is a peer that is not
    /// a trusted proxy; `Unknown` is no peer at all.
    pub(crate) fn peer_is_trusted(self) -> bool {
        matches!(self, Self::Forwarded(_) | Self::TrustedProxy(_))
    }

    /// The resolution itself, over plain values — the whole security argument
    /// of this module lives here, and so do its tests.
    pub fn resolve(
        forwarded_for: Option<&str>,
        real_ip: Option<&str>,
        peer: Option<IpAddr>,
        trusted_proxies: &[IpAddr],
    ) -> Self {
        let Some(peer) = peer else {
            return Self::Unknown;
        };
        // Anyone but a trusted proxy could have forged the headers, so the peer
        // is the client and nothing else is read.
        if !trusted_proxies.contains(&peer) {
            return Self::Peer(peer);
        }

        // Scanned lazily in both directions rather than collected: `split` on a
        // `char` is double-ended, so neither pass allocates on a path that runs
        // per request.
        let hops = || {
            forwarded_for
                .unwrap_or_default()
                .split(',')
                .filter_map(parse_forwarded_entry)
        };
        // The hop infrastructure appended most recently that is not itself ours.
        if let Some(client) = hops().rev().find(|ip| !trusted_proxies.contains(ip)) {
            return Self::Forwarded(client);
        }
        // nginx's single-hop form. Read after the chain because it carries no
        // ordering of its own, and skipped when it names a proxy we already
        // know is infrastructure.
        if let Some(ip) = real_ip
            .and_then(parse_forwarded_entry)
            .filter(|ip| !trusted_proxies.contains(ip))
        {
            return Self::Forwarded(ip);
        }
        // Degenerate: every recorded hop is itself a trusted proxy. The
        // outermost recorded address is still more specific than the peer.
        if let Some(outermost) = hops().next() {
            return Self::Forwarded(outermost);
        }
        Self::TrustedProxy(peer)
    }
}

/// Best-effort client IP for the current request. Always present — see
/// [`ClientOrigin`] for the resolution and the security caveat.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ClientIp {
    /// The resolved address. `0.0.0.0` only when the request has no peer
    /// address at all.
    pub ip: IpAddr,
    /// `true` when a trusted proxy's `X-Forwarded-For` / `X-Real-IP` supplied
    /// the address, `false` when it is the direct peer (or the default).
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

impl From<ClientOrigin> for ClientIp {
    fn from(origin: ClientOrigin) -> Self {
        match origin {
            ClientOrigin::Forwarded(ip) => Self {
                ip,
                forwarded: true,
            },
            ClientOrigin::Peer(ip) | ClientOrigin::TrustedProxy(ip) => Self {
                ip,
                forwarded: false,
            },
            ClientOrigin::Unknown => Self::unknown(),
        }
    }
}

/// Parse one forwarded entry — strip whitespace, accept a bare IP, `IP:port`,
/// or a bracketed IPv6 (`[ip]` or `[ip]:port` per RFC 7239 — nginx and HAProxy
/// emit the bracketed form). An unparseable entry yields `None` and is skipped
/// rather than accepted as a key, so a caller cannot inject an arbitrary string
/// into a rate-limit bucket name or a log field.
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

impl<'a> FromRequest<'a> for ClientIp {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        Ok(ClientOrigin::of(req).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test literal is an IP")
    }

    fn resolve(xff: Option<&str>, peer: Option<IpAddr>, trusted: &[IpAddr]) -> ClientOrigin {
        ClientOrigin::resolve(xff, None, peer, trusted)
    }

    // ── The peer gate ───────────────────────────────────────────────────────

    #[test]
    fn no_peer_address_is_unknown() {
        assert_eq!(
            resolve(Some("203.0.113.50"), None, &[]),
            ClientOrigin::Unknown,
        );
    }

    // The default deployment: nothing is trusted, so a header a caller set is
    // never read. This is the regression that mattered — the extractor used to
    // return the peer with `forwarded = false` for a *different* reason (a
    // `return` above the header branches), which read as correct behaviour
    // while making the trusted-proxy case unreachable too.
    #[test]
    fn an_untrusted_peer_ignores_forwarding_headers() {
        let origin = resolve(Some("203.0.113.50"), Some(ip("192.0.2.10")), &[]);
        assert_eq!(origin, ClientOrigin::Peer(ip("192.0.2.10")));

        // Even a well-formed multi-hop chain from an untrusted peer.
        let origin = resolve(
            Some("203.0.113.50, 10.0.0.99"),
            Some(ip("192.0.2.10")),
            &[ip("10.0.0.1")],
        );
        assert_eq!(origin, ClientOrigin::Peer(ip("192.0.2.10")));
    }

    // ── Behind a trusted proxy ──────────────────────────────────────────────

    // B-HTTP-1: the real client is the hop the proxy APPENDED (the rightmost
    // non-trusted), not the leftmost — the leftmost is the client-authored,
    // spoofable value.
    #[test]
    fn a_trusted_proxy_yields_the_rightmost_untrusted_hop() {
        let proxy = ip("10.0.0.1");
        // The proxy received the request from 192.0.2.1 and appended it; the
        // leftmost "203.0.113.50" is a header the client set.
        let origin = resolve(Some("203.0.113.50, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(origin, ClientOrigin::Forwarded(ip("192.0.2.1")));
    }

    // B-HTTP-1 (the core exploit): an attacker prepends a random or victim IP.
    // The genuine hop sits to its right and is the one selected, so the
    // prepended value can neither mint a fresh identity nor claim a victim's.
    #[test]
    fn a_prepended_spoofed_hop_cannot_change_the_answer() {
        let proxy = ip("10.0.0.1");
        let genuine = ClientOrigin::Forwarded(ip("203.0.113.50"));
        assert_eq!(
            resolve(Some("203.0.113.50"), Some(proxy), &[proxy]),
            genuine
        );
        assert_eq!(
            resolve(Some("1.2.3.4, 203.0.113.50"), Some(proxy), &[proxy]),
            genuine,
            "a rotating leading hop cannot mint a fresh identity",
        );
        assert_eq!(
            resolve(Some("198.51.100.7, 203.0.113.50"), Some(proxy), &[proxy]),
            genuine,
            "a forged victim address cannot be targeted",
        );
    }

    // A two-layer chain (LB → nginx → app): both infra hops are trusted, so the
    // client is the rightmost hop that is not one of them.
    #[test]
    fn a_two_layer_proxy_chain_selects_the_real_client() {
        let nginx = ip("10.0.0.1");
        let lb = ip("10.0.0.2");
        // client(203.0.113.50) → lb appended it → nginx appended lb.
        let origin = resolve(Some("203.0.113.50, 10.0.0.2"), Some(nginx), &[nginx, lb]);
        assert_eq!(origin, ClientOrigin::Forwarded(ip("203.0.113.50")));

        // And a spoofed hop prepended inside that chain is still skipped.
        let origin = resolve(
            Some("9.9.9.9, 203.0.113.50, 10.0.0.2"),
            Some(nginx),
            &[nginx, lb],
        );
        assert_eq!(origin, ClientOrigin::Forwarded(ip("203.0.113.50")));
    }

    #[test]
    fn an_unparseable_hop_is_skipped_never_used_as_a_key() {
        let proxy = ip("10.0.0.1");
        let origin = resolve(Some("not-an-ip, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(origin, ClientOrigin::Forwarded(ip("192.0.2.1")));
    }

    #[test]
    fn hop_parsing_accepts_the_shapes_real_proxies_emit() {
        let proxy = ip("10.0.0.1");
        let cases = [
            ("   203.0.113.50  ", "203.0.113.50"),
            ("203.0.113.42:51000", "203.0.113.42"),
            ("2001:db8::1", "2001:db8::1"),
            ("[2001:db8::1]", "2001:db8::1"),
            ("[2001:db8::1]:8080", "2001:db8::1"),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                resolve(Some(raw), Some(proxy), &[proxy]),
                ClientOrigin::Forwarded(ip(expected)),
                "hop {raw:?}",
            );
        }
        // Malformed shapes yield no hop at all.
        for raw in ["[malformed::]", "not-an-ip"] {
            assert_eq!(
                resolve(Some(raw), Some(proxy), &[proxy]),
                ClientOrigin::TrustedProxy(proxy),
                "hop {raw:?}",
            );
        }
    }

    #[test]
    fn x_real_ip_answers_when_the_chain_does_not() {
        let proxy = ip("10.0.0.1");
        let origin = ClientOrigin::resolve(None, Some("198.51.100.20"), Some(proxy), &[proxy]);
        assert_eq!(origin, ClientOrigin::Forwarded(ip("198.51.100.20")));

        // The chain outranks it when it carries a usable hop.
        let origin = ClientOrigin::resolve(
            Some("203.0.113.50"),
            Some("198.51.100.20"),
            Some(proxy),
            &[proxy],
        );
        assert_eq!(origin, ClientOrigin::Forwarded(ip("203.0.113.50")));

        // And an untrusted peer's X-Real-IP is ignored like everything else.
        let origin =
            ClientOrigin::resolve(None, Some("198.51.100.20"), Some(ip("192.0.2.10")), &[]);
        assert_eq!(origin, ClientOrigin::Peer(ip("192.0.2.10")));
    }

    // Degenerate: every recorded hop is itself a trusted proxy — the outermost
    // recorded address is still more specific than the peer.
    #[test]
    fn an_all_trusted_chain_falls_back_to_the_outermost_hop() {
        let (a, b, c) = (ip("10.0.0.1"), ip("10.0.0.2"), ip("10.0.0.3"));
        let origin = resolve(Some("10.0.0.2, 10.0.0.3"), Some(a), &[a, b, c]);
        assert_eq!(origin, ClientOrigin::Forwarded(b));
    }

    #[test]
    fn a_trusted_proxy_that_forwards_nothing_is_reported_as_such() {
        let proxy = ip("10.0.0.1");
        for chain in [None, Some(""), Some(",,,")] {
            assert_eq!(
                resolve(chain, Some(proxy), &[proxy]),
                ClientOrigin::TrustedProxy(proxy),
                "chain {chain:?}",
            );
        }
    }

    // ── The extractor over the resolution ───────────────────────────────────

    /// A request carrying a real transport peer — what a built `Request` lacks,
    /// and what every branch past `Unknown` needs. `poem`'s builder cannot set
    /// one, so the parts are assembled directly.
    fn req_from(peer: &str, headers: &[(&str, &str)]) -> Request {
        use poem::Addr;
        use poem::web::{LocalAddr, RemoteAddr};

        let socket: SocketAddr = format!("{peer}:54321").parse().expect("test literal");
        let (mut parts, _) = poem::http::Request::new(()).into_parts();
        for (name, value) in headers {
            parts.headers.insert(
                poem::http::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                poem::http::HeaderValue::from_str(value).expect("header value"),
            );
        }
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

    /// Extract under an ambient request scope over a container holding `config`
    /// — the shape the transport edge installs, and the only place the
    /// trusted-proxy list comes from.
    async fn extract_under(config: Option<HttpConfig>, req: Request) -> ClientIp {
        let mut builder = nest_rs_core::Container::builder();
        if let Some(config) = config {
            builder = builder.provide(config);
        }
        let scope = std::sync::Arc::new(nest_rs_core::RequestScope::new(builder.build()));
        nest_rs_core::with_request_scope(
            Some(scope),
            nest_rs_core::Correlation::mint(),
            None,
            extract(req),
        )
        .await
    }

    fn trusting(proxies: &[&str]) -> HttpConfig {
        HttpConfig {
            trusted_proxies: proxies.iter().map(|p| ip(p)).collect(),
            ..HttpConfig::default()
        }
    }

    // The wiring, end to end: a peer, a forwarding header, and an `HttpConfig`
    // naming that peer. Every unit above tests the rule; this tests that the
    // extractor actually reaches it.
    #[tokio::test]
    async fn the_extractor_reads_the_trusted_proxies_off_the_booted_config() {
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        let extracted = extract_under(Some(trusting(&["10.0.0.1"])), req).await;
        assert_eq!(extracted.ip, ip("203.0.113.50"));
        assert!(extracted.forwarded);
    }

    // The default deployment — `trusted_proxies` empty, so the same request
    // resolves to the peer. This is the regression: the extractor used to
    // answer the peer here *and* in the case above, because the header branches
    // were unreachable on TCP.
    #[tokio::test]
    async fn an_empty_trusted_proxy_list_resolves_the_same_request_to_the_peer() {
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        let extracted = extract_under(Some(HttpConfig::default()), req).await;
        assert_eq!(extracted.ip, ip("10.0.0.1"));
        assert!(!extracted.forwarded);
    }

    // No config in reach at all (a hand-built request off the transport task):
    // the same answer an unconfigured deployment gives, never a panic.
    #[tokio::test]
    async fn no_reachable_config_trusts_nothing() {
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        assert_eq!(extract_under(None, req).await.ip, ip("10.0.0.1"));
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        assert_eq!(extract(req).await.ip, ip("10.0.0.1"), "and off any scope");
    }

    async fn extract(req: Request) -> ClientIp {
        let (req, mut body) = req.split();
        ClientIp::from_request(&req, &mut body)
            .await
            .expect("the extractor is infallible")
    }

    #[tokio::test]
    async fn a_request_with_no_peer_and_no_trusted_proxy_extracts_the_default() {
        // A built `Request` has no peer socket — the `Unknown` branch.
        let ip = extract(Request::builder().finish()).await;
        assert_eq!(ip, ClientIp::unknown());
    }

    // The end-to-end shape of the bug: headers present, no trusted proxy
    // declared, so the extractor reports the peer and says so.
    #[tokio::test]
    async fn headers_alone_never_set_forwarded() {
        let req = Request::builder()
            .header("x-forwarded-for", "9.9.9.9")
            .header("x-real-ip", "9.9.9.9")
            .finish();
        let extracted = extract(req).await;
        assert_eq!(extracted, ClientIp::unknown());
        assert!(
            !extracted.forwarded,
            "an unverified header must never be reported as a forwarded address",
        );
    }

    #[test]
    fn forwarded_is_set_only_by_the_forwarded_variant() {
        let addr = ip("203.0.113.50");
        assert!(ClientIp::from(ClientOrigin::Forwarded(addr)).forwarded);
        assert!(!ClientIp::from(ClientOrigin::Peer(addr)).forwarded);
        assert!(!ClientIp::from(ClientOrigin::TrustedProxy(addr)).forwarded);
        assert_eq!(
            ClientIp::from(ClientOrigin::TrustedProxy(addr)).ip,
            addr,
            "an address is still reported, it is just not a client's",
        );
        assert_eq!(ClientIp::from(ClientOrigin::Unknown), ClientIp::unknown());
    }
}
