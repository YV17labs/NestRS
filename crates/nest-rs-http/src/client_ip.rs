//! Who the request came from — [`ClientOrigin`], the one resolution every
//! consumer shares, and [`ClientIp`], the extractor over it.
//!
//! The forwarding headers are client-authored, so they are read only when the
//! direct peer is in `<PREFIX>_HTTP__TRUSTED_PROXIES` (empty by default), an
//! address or a CIDR range ([`TrustedProxy`]):
//!
//! 1. no peer address at all (unix socket, or a proxy that hides it) ⇒
//!    [`ClientOrigin::Unknown`];
//! 2. an untrusted peer *is* the client ([`ClientOrigin::Peer`]), headers ignored;
//! 3. a trusted peer ⇒ `Forwarded` (RFC 7239), `X-Forwarded-For`, then
//!    `X-Real-IP` are read, and the client is the **rightmost** hop that is not
//!    itself a trusted proxy ([`ClientOrigin::Forwarded`]): a proxy appends, a
//!    caller can only prepend, on one field line or over several.
//!
//! A header is read whole or not at all, and the headers agree or none is
//! believed: one the parser cannot read, a client position holding a node the
//! proxy withheld (`unknown`, an obfuscated `_id`), or two headers naming
//! different clients name no client whatever the others say, and the trusted
//! proxy stands ([`ClientOrigin::TrustedProxy`]). `X-Real-IP`, one hop in no
//! order, answers alone only where neither chain names a client: behind a proxy
//! writing one header and passing the others through, the one a caller wrote
//! would otherwise choose.
//!
//! An IPv4-mapped address, what a dual-stack listener reports an IPv4 peer as,
//! is the IPv4 address it carries, in the peer, in a hop and in the answer.
//!
//! [`ClientIp`] and the throttler's bucket both resolve through
//! [`ClientOrigin::of`]. Treat the result as observational, never as an authn
//! or authz input; the address is personal data, so no line or span carries it.

use std::borrow::Cow;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use http::header::AsHeaderName;
use http::{HeaderMap, header};
use nest_rs_core::current_request_scope;

use crate::HttpConfig;
use crate::trusted_proxy::{TrustedProxy, lists};

/// The de-facto forwarding headers, which the `http` crate does not name.
const X_FORWARDED_FOR: &str = "x-forwarded-for";
const X_REAL_IP: &str = "x-real-ip";

/// Who a request is attributed to, and on what evidence.
/// [`Unknown`](Self::Unknown) and [`TrustedProxy`](Self::TrustedProxy) collapse
/// every caller into one throttler bucket.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum ClientOrigin {
    /// The direct transport peer, which is not a configured trusted proxy.
    /// Forwarding headers were present or not — either way they were ignored.
    Peer(IpAddr),
    /// A hop read from `Forwarded` / `X-Forwarded-For` / `X-Real-IP`, admitted
    /// because the direct peer is a configured trusted proxy.
    Forwarded(IpAddr),
    /// The peer is a trusted proxy but forwarded no usable client address: none,
    /// one it withheld, two headers naming different ones, or a header the
    /// parser cannot read whole. Every caller behind it is indistinguishable.
    TrustedProxy(IpAddr),
    /// No peer address at all — a unix socket, or a proxy that hides it.
    Unknown,
}

impl ClientOrigin {
    /// The address the request is attributed to — the peer, a forwarded hop,
    /// or the trusted proxy that forwarded none — or `None` when there is no
    /// peer address at all.
    pub const fn ip(self) -> Option<IpAddr> {
        match self {
            Self::Peer(ip) | Self::Forwarded(ip) | Self::TrustedProxy(ip) => Some(ip),
            Self::Unknown => None,
        }
    }

    /// The resolution the transport made of a live request, against the
    /// trusted-proxy list of the [`HttpConfig`] the app booted with.
    ///
    /// Off the transport (a hand-built `Request`) it resolves again, against the
    /// config of the request's app; off the request task nothing is trusted.
    pub fn of(req: &crate::Request) -> Self {
        if let Some(origin) = req.extensions().get::<Self>() {
            return *origin;
        }
        let config = configured();
        Self::filed(Self::of_with(req, trusted_proxies(config.as_deref())))
    }

    /// [`of`](Self::of) against an explicit list, for the transport edge, which
    /// runs before the request scope `of` reads exists, and before the request's
    /// trace context: what it refused to believe is the edge's to file inside it.
    pub(crate) fn of_with(
        req: &crate::Request,
        trusted_proxies: &[TrustedProxy],
    ) -> (Self, Option<Refusal>) {
        Self::of_head(
            req.headers(),
            req.peer_addr().map(|peer| peer.ip()),
            trusted_proxies,
        )
    }

    /// The resolution over a request's head and its peer.
    fn of_head(
        headers: &HeaderMap,
        peer: Option<IpAddr>,
        trusted_proxies: &[TrustedProxy],
    ) -> (Self, Option<Refusal>) {
        // A peer off the list could have forged them all, so none is read.
        if !peer.is_some_and(|peer| lists(trusted_proxies, peer)) {
            return Self::weigh(None, None, None, peer, trusted_proxies);
        }
        Self::weigh(
            field_list(headers, header::FORWARDED).as_deref(),
            field_list(headers, X_FORWARDED_FOR).as_deref(),
            // One hop, no list: the line a proxy adds is the last.
            headers
                .get_all(X_REAL_IP)
                .iter()
                .next_back()
                .map(|line| String::from_utf8_lossy(line.as_bytes()))
                .as_deref(),
            peer,
            trusted_proxies,
        )
    }

    /// The client's address, when the evidence names one: the direct peer, or
    /// the hop a trusted proxy forwarded — never the proxy's own.
    pub(crate) fn client(self) -> Option<IpAddr> {
        match self {
            Self::Peer(ip) | Self::Forwarded(ip) => Some(ip),
            Self::TrustedProxy(_) | Self::Unknown => None,
        }
    }

    /// Whether the direct peer is declared infrastructure — the one fact that
    /// decides whether *any* header this caller sent may be believed, the
    /// `X-Request-Id` gate included.
    pub(crate) fn peer_is_trusted(self) -> bool {
        matches!(self, Self::Forwarded(_) | Self::TrustedProxy(_))
    }

    /// The resolution itself, over plain values. `forwarded` is the RFC 7239
    /// field value, `forwarded_for` and `real_ip` the de-facto pair; a list sent
    /// over several field lines is passed as one, in order (RFC 9110 §5.3).
    pub fn resolve(
        forwarded: Option<&str>,
        forwarded_for: Option<&str>,
        real_ip: Option<&str>,
        peer: Option<IpAddr>,
        trusted_proxies: &[TrustedProxy],
    ) -> Self {
        Self::filed(Self::weigh(
            forwarded,
            forwarded_for,
            real_ip,
            peer,
            trusted_proxies,
        ))
    }

    /// The origin a resolution answered, what it refused to believe filed where
    /// it was read.
    fn filed((origin, refusal): (Self, Option<Refusal>)) -> Self {
        if let Some(refusal) = refusal {
            refusal.file();
        }
        origin
    }

    /// [`resolve`](Self::resolve), and what it refused to believe, unfiled.
    fn weigh(
        forwarded: Option<&str>,
        forwarded_for: Option<&str>,
        real_ip: Option<&str>,
        peer: Option<IpAddr>,
        trusted_proxies: &[TrustedProxy],
    ) -> (Self, Option<Refusal>) {
        let Some(peer) = peer.map(|peer| peer.to_canonical()) else {
            return (Self::Unknown, None);
        };
        // Anyone but a trusted proxy could have forged the headers.
        if !lists(trusted_proxies, peer) {
            return (Self::Peer(peer), None);
        }
        // A header read two ways names no client, and decides for every
        // header: ignored alone, it would leave the answer to one a caller wrote.
        let refused = |reason| (Self::TrustedProxy(peer), Some(Refusal { peer, reason }));

        let Ok(standard) = Chain::of(
            ForwardedPairs::new(forwarded.unwrap_or_default()).filter_map(|pair| match pair {
                Ok((name, value)) => name.eq_ignore_ascii_case("for").then(|| Hop::parse(value)),
                Err(unreadable) => Some(Err(unreadable)),
            }),
            trusted_proxies,
        ) else {
            return refused(Reason::Unreadable("forwarded"));
        };
        let Ok(de_facto) = Chain::of(
            forwarded_for
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|element| !element.is_empty())
                .map(Hop::parse),
            trusted_proxies,
        ) else {
            return refused(Reason::Unreadable(X_FORWARDED_FOR));
        };
        // nginx's single-hop form: one hop in no order, read whole like the
        // chains. A hop of ours names no client.
        let single = match real_ip
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(Hop::parse)
        {
            Some(Err(Unreadable)) => return refused(Reason::Unreadable(X_REAL_IP)),
            Some(Ok(Hop::Address(ip))) if lists(trusted_proxies, ip) => None,
            Some(Ok(hop)) => Some(hop),
            None => None,
        };
        let mut client = None;
        let mut withheld = false;
        for hop in [standard.client, de_facto.client, single]
            .into_iter()
            .flatten()
        {
            match hop {
                // Headers naming different clients are all refused (RFC 7239
                // §8.1): behind a proxy writing one and passing the others
                // through (nginx appending `X-Forwarded-For` beside a caller's
                // `Forwarded`, or writing `X-Real-IP` beside a caller's
                // `X-Forwarded-For`), the one a caller wrote would decide.
                Hop::Address(ip) if client.is_some_and(|named| named != ip) => {
                    return refused(Reason::Disagreement);
                }
                Hop::Address(ip) => client = Some(ip),
                Hop::Withheld => withheld = true,
            }
        }
        // The hop facing the client withheld it: what sits left of it, or in
        // another header, is the caller's own to write.
        if withheld {
            return (Self::TrustedProxy(peer), None);
        }
        if let Some(client) = client {
            return (Self::Forwarded(client), None);
        }
        // Degenerate: every recorded hop is itself a trusted proxy. The
        // outermost recorded address is still more specific than the peer.
        match standard.outermost.or(de_facto.outermost) {
            Some(outermost) => (Self::Forwarded(outermost), None),
            None => (Self::TrustedProxy(peer), None),
        }
    }
}

/// What a resolution refused to believe from a trusted proxy, filed once,
/// inside the request it was refused for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal {
    peer: IpAddr,
    reason: Reason,
}

/// Why no client was believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// The header the parser could not read whole.
    Unreadable(&'static str),
    /// Two headers naming different clients.
    Disagreement,
}

impl Refusal {
    /// The `warn` an operator reads: the proxy is named, never what it relayed,
    /// which may be a caller's own and is personal data.
    pub(crate) fn file(&self) {
        match self.reason {
            Reason::Unreadable(header) => tracing::warn!(
                target: crate::target::HTTP,
                peer = %self.peer,
                header,
                "forwarding header unreadable — no client is believed",
            ),
            Reason::Disagreement => tracing::warn!(
                target: crate::target::HTTP,
                peer = %self.peer,
                "forwarding headers disagree about the client — neither is believed",
            ),
        }
    }
}

/// Best-effort client IP for the current request. Always present — see
/// [`ClientOrigin`] for the resolution and the security caveat.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub struct ClientIp {
    /// The resolved address. `0.0.0.0` only when the request has no peer
    /// address at all.
    pub ip: IpAddr,
    /// `true` when a trusted proxy's forwarding header supplied the address,
    /// `false` when it is the direct peer (or the default).
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

/// Every field line of the list `name`, in order, as one field value (RFC 9110
/// §5.3): a proxy may add its own line after the caller's, so the first alone
/// is the caller's to choose. Bytes no text holds are replaced line by line,
/// and a hop holding them is no node, so the list is unreadable.
fn field_list(headers: &HeaderMap, name: impl AsHeaderName) -> Option<Cow<'_, str>> {
    let mut lines = headers
        .get_all(name)
        .into_iter()
        .map(|line| String::from_utf8_lossy(line.as_bytes()));
    let mut list = lines.next()?;
    for line in lines {
        let list = list.to_mut();
        list.push_str(", ");
        list.push_str(&line);
    }
    Some(list)
}

/// A forwarding header the parser cannot read whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unreadable;

/// One hop of a forwarding chain: the address a proxy recorded, or a node it
/// withheld (RFC 7239 §6.2 `unknown`, §6.3 an obfuscated identifier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hop {
    Address(IpAddr),
    Withheld,
}

impl Hop {
    /// One RFC 7239 `node` (§6), or one de-facto list element: an address bare,
    /// with a port, or bracketed (`[ip]`, `[ip]:port`, the form nginx and
    /// HAProxy emit for IPv6), `unknown`, or an obfuscated `_id`, each in
    /// canonical form. Anything else is no node, so its list is no chain: a hop
    /// a caller could not have the parser skip is never a rate-limit key either.
    fn parse(node: &str) -> Result<Self, Unreadable> {
        if let Ok(ip) = node.parse::<IpAddr>() {
            return Ok(Self::Address(ip.to_canonical()));
        }
        if let Some(bracketed) = node.strip_prefix('[') {
            let (inside, after) = bracketed.split_once(']').ok_or(Unreadable)?;
            let ip = inside.parse::<Ipv6Addr>().ok().ok_or(Unreadable)?;
            if !(after.is_empty() || after.strip_prefix(':').is_some_and(is_port)) {
                return Err(Unreadable);
            }
            return Ok(Self::Address(IpAddr::V6(ip).to_canonical()));
        }
        let name = match node.rsplit_once(':') {
            Some((name, port)) if is_port(port) => name,
            Some(_) => return Err(Unreadable),
            None => node,
        };
        if name.eq_ignore_ascii_case("unknown") || is_obfuscated(name) {
            return Ok(Self::Withheld);
        }
        name.parse::<Ipv4Addr>()
            .ok()
            .map(|ip| Self::Address(IpAddr::V4(ip)))
            .ok_or(Unreadable)
    }
}

/// RFC 7239 §6's `node-port`: a port number, or an obfuscated one.
fn is_port(port: &str) -> bool {
    // `u16`'s parser takes a leading `+`, which no port writes.
    is_obfuscated(port) || (port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok())
}

/// RFC 7239 §6.3's `obfnode` and `obfport`: `_` and one or more letters, digits,
/// `.`, `_` or `-`.
fn is_obfuscated(text: &str) -> bool {
    text.strip_prefix('_').is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    })
}

/// What one forwarding chain says: the rightmost hop that is not ours (the
/// client), and the outermost recorded hop (the degenerate answer, kept for
/// when every hop is infrastructure), both from a single forward pass that
/// reads the whole chain.
#[derive(Debug, Default, Clone, Copy)]
struct Chain {
    client: Option<Hop>,
    outermost: Option<IpAddr>,
}

impl Chain {
    fn of(
        hops: impl Iterator<Item = Result<Hop, Unreadable>>,
        trusted_proxies: &[TrustedProxy],
    ) -> Result<Self, Unreadable> {
        let mut chain = Self::default();
        for hop in hops {
            let hop = hop?;
            if let Hop::Address(ip) = hop {
                chain.outermost.get_or_insert(ip);
                if lists(trusted_proxies, ip) {
                    continue;
                }
            }
            chain.client = Some(hop);
        }
        Ok(chain)
    }
}

/// The first `name=` parameter of an RFC 7239 `Forwarded` field value, unquoted:
/// the one the hop facing the client wrote, read up to the first byte the
/// grammar refuses.
pub(crate) fn forwarded_param<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    ForwardedPairs::new(raw)
        .map_while(Result::ok)
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

/// The `token "=" ( token / quoted-string )` pairs of a `Forwarded` value, in
/// order, a value's surrounding quotes removed and a quoted-pair left escaped
/// (RFC 7239 §4), then [`Unreadable`] once at the first byte the grammar refuses.
///
/// What it refuses is what reads two ways: a quote that no quote closes, one
/// inside a value, a closing quote followed by anything but a separator, a pair
/// with no `=` or no name, an empty value. A proxy appending a quoted value to a
/// caller's line that left a quote open would otherwise close the caller's
/// quote, and the caller's hop would read as the rightmost. Whitespace around a
/// separator and a value outside `tchar` (an unquoted `[::1]:80` a hand-written
/// proxy rule emits) read one way, and pass.
struct ForwardedPairs<'a> {
    rest: Option<&'a str>,
}

impl<'a> ForwardedPairs<'a> {
    fn new(raw: &'a str) -> Self {
        Self { rest: Some(raw) }
    }

    /// The pair `text` starts with and what follows it, from its separator on.
    fn pair(text: &'a str) -> Result<((&'a str, &'a str), &'a str), Unreadable> {
        let equals = text.find(['=', ',', ';', '"']).ok_or(Unreadable)?;
        let name = text[..equals].trim_end_matches(OWS);
        if text.as_bytes()[equals] != b'=' || name.is_empty() || name.contains(OWS) {
            return Err(Unreadable);
        }
        let value = text[equals + 1..].trim_start_matches(OWS);
        if let Some(quoted) = value.strip_prefix('"') {
            let close = closing_quote(quoted).ok_or(Unreadable)?;
            let rest = quoted[close + 1..].trim_start_matches(OWS);
            if !(rest.is_empty() || rest.starts_with([',', ';'])) {
                return Err(Unreadable);
            }
            return Ok(((name, &quoted[..close]), rest));
        }
        let end = value.find([',', ';']).unwrap_or(value.len());
        let token = value[..end].trim_end_matches(OWS);
        if token.is_empty() || token.contains('"') {
            return Err(Unreadable);
        }
        Ok(((name, token), &value[end..]))
    }
}

impl<'a> Iterator for ForwardedPairs<'a> {
    type Item = Result<(&'a str, &'a str), Unreadable>;

    fn next(&mut self) -> Option<Self::Item> {
        // An empty element (`for=a,,for=b`) or pair (`a=b;;c=d`) is legal and
        // carries nothing.
        let text = self.rest?.trim_start_matches([' ', '\t', ',', ';']);
        if text.is_empty() {
            self.rest = None;
            return None;
        }
        let read = Self::pair(text);
        // Past a refusal nothing is read.
        self.rest = read.as_ref().ok().map(|&(_, rest)| rest);
        Some(read.map(|(pair, _)| pair))
    }
}

/// Optional whitespace (RFC 9110 §5.6.3).
const OWS: [char; 2] = [' ', '\t'];

/// Where the `quoted-string` whose opening quote precedes `quoted` closes: its
/// first quote no backslash escapes.
fn closing_quote(quoted: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in quoted.bytes().enumerate() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            b'"' => return Some(index),
            _ => {}
        }
    }
    None
}

impl<'a> crate::FromRequest<'a> for ClientIp {
    async fn from_request(
        req: &'a crate::Request,
        _body: &mut crate::RequestBody,
    ) -> crate::Result<Self> {
        Ok(ClientOrigin::of(req).into())
    }
}

/// The configuration of the app the running request belongs to; none off the
/// request task.
fn configured() -> Option<std::sync::Arc<HttpConfig>> {
    current_request_scope().and_then(|scope| scope.root().get::<HttpConfig>())
}

/// The proxies `config` trusts; none without one.
pub(crate) fn trusted_proxies(config: Option<&HttpConfig>) -> &[TrustedProxy] {
    config.map_or(&[][..], |config| &config.trusted_proxies)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;
    use crate::Request;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test literal is an IP")
    }

    fn listed(addresses: &[IpAddr]) -> Vec<TrustedProxy> {
        addresses.iter().copied().map(TrustedProxy::from).collect()
    }

    fn resolve(xff: Option<&str>, peer: Option<IpAddr>, trusted: &[IpAddr]) -> ClientOrigin {
        ClientOrigin::resolve(None, xff, None, peer, &listed(trusted))
    }

    #[test]
    fn two_forwarding_headers_that_disagree_are_both_refused() {
        let proxy: IpAddr = "10.0.0.1".parse().unwrap();
        let trusted = listed(&[proxy]);

        let logs = nest_rs_testing::LogCapture::install();
        let spoofed = ClientOrigin::resolve(
            Some("for=198.51.100.9"),
            Some("203.0.113.7"),
            None,
            Some(proxy),
            &trusted,
        );
        assert_eq!(
            spoofed,
            ClientOrigin::TrustedProxy(proxy),
            "a disagreement names no client, so neither header decides",
        );
        let reported = logs.expect_one(crate::target::HTTP, DISAGREEMENT);
        assert_eq!(reported.level, "warn");
        assert_eq!(reported.field("peer").as_deref(), Some("10.0.0.1"));
        for claimed in ["198.51.100.9", "203.0.113.7"] {
            assert!(
                reported
                    .fields
                    .values()
                    .all(|value| !value.contains(claimed)),
                "{:?}",
                reported.fields
            );
        }
        drop(logs);

        let agreeing = ClientOrigin::resolve(
            Some("for=203.0.113.7"),
            Some("203.0.113.7"),
            None,
            Some(proxy),
            &trusted,
        );
        assert_eq!(
            agreeing,
            ClientOrigin::Forwarded("203.0.113.7".parse().unwrap())
        );

        for (fwd, xff) in [(Some("for=203.0.113.7"), None), (None, Some("203.0.113.7"))] {
            assert_eq!(
                ClientOrigin::resolve(fwd, xff, None, Some(proxy), &trusted),
                ClientOrigin::Forwarded("203.0.113.7".parse().unwrap()),
            );
        }
    }

    fn resolve_forwarded(
        forwarded: &str,
        peer: Option<IpAddr>,
        trusted: &[IpAddr],
    ) -> ClientOrigin {
        ClientOrigin::resolve(Some(forwarded), None, None, peer, &listed(trusted))
    }

    #[test]
    fn no_peer_address_is_unknown() {
        assert_eq!(
            resolve(Some("203.0.113.50"), None, &[]),
            ClientOrigin::Unknown,
        );
    }

    #[test]
    fn every_origin_but_an_unknown_one_is_attributed_to_its_address() {
        let addr = ip("203.0.113.50");
        for origin in [
            ClientOrigin::Peer(addr),
            ClientOrigin::Forwarded(addr),
            ClientOrigin::TrustedProxy(addr),
        ] {
            assert_eq!(origin.ip(), Some(addr), "{origin:?}");
        }
        assert_eq!(ClientOrigin::Unknown.ip(), None);
    }

    #[test]
    fn an_untrusted_peer_ignores_forwarding_headers() {
        let origin = resolve(Some("203.0.113.50"), Some(ip("192.0.2.10")), &[]);
        assert_eq!(origin, ClientOrigin::Peer(ip("192.0.2.10")));

        let origin = resolve(
            Some("203.0.113.50, 10.0.0.99"),
            Some(ip("192.0.2.10")),
            &[ip("10.0.0.1")],
        );
        assert_eq!(origin, ClientOrigin::Peer(ip("192.0.2.10")));
    }

    #[test]
    fn a_trusted_proxy_yields_the_rightmost_untrusted_hop() {
        let proxy = ip("10.0.0.1");
        // The leftmost hop is the one the client set.
        let origin = resolve(Some("203.0.113.50, 192.0.2.1"), Some(proxy), &[proxy]);
        assert_eq!(origin, ClientOrigin::Forwarded(ip("192.0.2.1")));
    }

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

    #[test]
    fn a_two_layer_proxy_chain_selects_the_real_client() {
        let nginx = ip("10.0.0.1");
        let lb = ip("10.0.0.2");
        // client → lb appended it → nginx appended lb.
        let origin = resolve(Some("203.0.113.50, 10.0.0.2"), Some(nginx), &[nginx, lb]);
        assert_eq!(origin, ClientOrigin::Forwarded(ip("203.0.113.50")));

        let origin = resolve(
            Some("9.9.9.9, 203.0.113.50, 10.0.0.2"),
            Some(nginx),
            &[nginx, lb],
        );
        assert_eq!(origin, ClientOrigin::Forwarded(ip("203.0.113.50")));
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
        let origin = ClientOrigin::resolve(
            None,
            None,
            Some("198.51.100.20"),
            Some(proxy),
            &listed(&[proxy]),
        );
        assert_eq!(origin, ClientOrigin::Forwarded(ip("198.51.100.20")));

        let origin = ClientOrigin::resolve(
            None,
            Some("10.0.0.1"),
            Some("198.51.100.20"),
            Some(proxy),
            &listed(&[proxy]),
        );
        assert_eq!(
            origin,
            ClientOrigin::Forwarded(ip("198.51.100.20")),
            "a chain of the proxies' own names no client",
        );

        let origin = ClientOrigin::resolve(
            None,
            None,
            Some("198.51.100.20"),
            Some(ip("192.0.2.10")),
            &[],
        );
        assert_eq!(origin, ClientOrigin::Peer(ip("192.0.2.10")));
    }

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

    #[test]
    fn the_rfc_7239_examples_resolve_to_their_for_node() {
        let proxy = ip("10.0.0.1");
        let cases = [
            // §4, the header's introductory example.
            ("for=\"_gazonk\"", None),
            // §4 again — an IPv6 node is bracketed *and* quoted.
            (
                "For=\"[2001:db8:cafe::17]:4711\"",
                Some("2001:db8:cafe::17"),
            ),
            // §4, several parameters in one element.
            (
                "for=192.0.2.60;proto=http;by=203.0.113.43",
                Some("192.0.2.60"),
            ),
            // §4, two elements — the rightmost is the hop appended last.
            ("for=192.0.2.43, for=198.51.100.17", Some("198.51.100.17")),
            // §7.1.
            ("FOR=\"192.0.2.43:47011\"", Some("192.0.2.43")),
        ];
        let logs = nest_rs_testing::LogCapture::install();
        for (header, expected) in cases {
            let origin = resolve_forwarded(header, Some(proxy), &[proxy]);
            let expected = match expected {
                Some(addr) => ClientOrigin::Forwarded(ip(addr)),
                None => ClientOrigin::TrustedProxy(proxy),
            };
            assert_eq!(origin, expected, "Forwarded: {header}");
        }
        assert!(
            logs.events_at(tracing::Level::WARN).is_empty(),
            "a value the RFC writes is read whole"
        );
    }

    /// A proxy rule written by hand may leave a node unquoted, outside `tchar`:
    /// it reads one way, so it is read.
    #[test]
    fn an_unquoted_node_a_hand_written_rule_emits_is_read() {
        let proxy = ip("10.0.0.1");
        for (header, client) in [
            ("for=2001:db8::1", "2001:db8::1"),
            ("for=[2001:db8::1]:8080", "2001:db8::1"),
            ("for=192.0.2.43:47011; proto=https", "192.0.2.43"),
            ("for = 192.0.2.43 , for=203.0.113.50", "203.0.113.50"),
        ] {
            assert_eq!(
                resolve_forwarded(header, Some(proxy), &[proxy]),
                ClientOrigin::Forwarded(ip(client)),
                "Forwarded: {header}",
            );
        }
    }

    /// `unknown` and an obfuscated identifier name no address, so they key no
    /// bucket; left of the client, a node the caller wrote decides nothing.
    #[test]
    fn unknown_and_obfuscated_nodes_name_no_client() {
        let proxy = ip("10.0.0.1");
        for (forwarded, client) in [
            ("for=unknown", None),
            ("for=_hidden", None),
            ("for=\"unknown:4711\"", None),
            ("for=unknown, for=203.0.113.50", Some("203.0.113.50")),
            (
                "for=_hidden, for=\"203.0.113.50:_port\"",
                Some("203.0.113.50"),
            ),
        ] {
            assert_eq!(
                resolve_forwarded(forwarded, Some(proxy), &[proxy]),
                client.map_or(ClientOrigin::TrustedProxy(proxy), |client| {
                    ClientOrigin::Forwarded(ip(client))
                }),
                "{forwarded}",
            );
        }
    }

    #[test]
    fn a_prepended_forwarded_element_cannot_change_the_answer() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            resolve_forwarded("for=198.51.100.7, for=203.0.113.50", Some(proxy), &[proxy],),
            ClientOrigin::Forwarded(ip("203.0.113.50")),
        );
    }

    #[test]
    fn an_untrusted_peer_ignores_the_forwarded_header_too() {
        let origin = resolve_forwarded("for=203.0.113.50", Some(ip("192.0.2.10")), &[]);
        assert_eq!(origin, ClientOrigin::Peer(ip("192.0.2.10")));
    }

    #[test]
    fn a_forwarded_chain_follows_the_same_trust_rules_as_the_de_facto_one() {
        let (nginx, lb) = (ip("10.0.0.1"), ip("10.0.0.2"));
        assert_eq!(
            resolve_forwarded("for=203.0.113.50, for=10.0.0.2", Some(nginx), &[nginx, lb],),
            ClientOrigin::Forwarded(ip("203.0.113.50")),
        );
        assert_eq!(
            resolve_forwarded(
                "for=10.0.0.2, for=10.0.0.3",
                Some(nginx),
                &[nginx, lb, ip("10.0.0.3")]
            ),
            ClientOrigin::Forwarded(lb),
            "every hop is infrastructure ⇒ the outermost recorded address",
        );
    }

    #[test]
    fn the_standard_header_does_not_outrank_the_de_facto_one() {
        let proxy = ip("10.0.0.1");
        let origin = ClientOrigin::resolve(
            Some("for=203.0.113.50"),
            Some("198.51.100.7"),
            Some("198.51.100.8"),
            Some(proxy),
            &listed(&[proxy]),
        );
        assert_eq!(
            origin,
            ClientOrigin::TrustedProxy(proxy),
            "a disagreement names no client rather than picking a header",
        );
    }

    #[test]
    fn a_quoted_value_carrying_a_separator_is_one_pair() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            resolve_forwarded(
                "by=\"[2001:db8::a;b]\";for=\"[2001:db8::1]:4711\"",
                Some(proxy),
                &[proxy],
            ),
            ClientOrigin::Forwarded(ip("2001:db8::1")),
        );
    }

    /// A quoted-pair closes nothing: a value a proxy quoted holds what follows
    /// it, a forged `for` included.
    #[test]
    fn an_escaped_quote_closes_no_value() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            resolve_forwarded(
                "for=203.0.113.7;host=\"\\\",for=198.51.100.98,\"",
                Some(proxy),
                &[proxy],
            ),
            ClientOrigin::Forwarded(ip("203.0.113.7")),
        );
    }

    #[test]
    fn a_malformed_forwarded_value_yields_no_hop() {
        let proxy = ip("10.0.0.1");
        for raw in [
            "",
            "for=",
            "for=;",
            "proto=https",
            "garbage",
            "for=\"unterminated",
        ] {
            assert_eq!(
                resolve_forwarded(raw, Some(proxy), &[proxy]),
                ClientOrigin::TrustedProxy(proxy),
                "Forwarded: {raw:?}",
            );
        }
    }

    /// What a listed peer's head resolves to, its field lines in the order sent.
    fn from_the_proxy(lines: &[(&str, &[u8])]) -> ClientOrigin {
        let proxy = ip("10.0.0.1");
        let mut headers = HeaderMap::new();
        for (name, value) in lines {
            headers.append(
                http::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                http::HeaderValue::from_bytes(value).expect("header value"),
            );
        }
        ClientOrigin::filed(ClientOrigin::of_head(
            &headers,
            Some(proxy),
            &listed(&[proxy]),
        ))
    }

    /// A proxy may add its own line after the caller's (RFC 7239 §4), and the
    /// lines of one name are one list (RFC 9110 §5.3): the rightmost hop is the
    /// proxy's on either spelling, so the caller's first line decides nothing.
    #[test]
    fn a_chain_over_several_field_lines_is_one_list() {
        for name in ["x-forwarded-for", "forwarded"] {
            let hop = |address: &str| {
                if name == "forwarded" {
                    format!("for={address}").into_bytes()
                } else {
                    address.as_bytes().to_vec()
                }
            };
            let (forged, client, inner) =
                (hop("198.51.100.99"), hop("203.0.113.7"), hop("10.0.0.1"));
            assert_eq!(
                from_the_proxy(&[(name, &forged), (name, &client)]),
                ClientOrigin::Forwarded(ip("203.0.113.7")),
                "{name}: the proxy's line follows the caller's",
            );
            assert_eq!(
                from_the_proxy(&[(name, &forged), (name, &client), (name, &inner)]),
                ClientOrigin::Forwarded(ip("203.0.113.7")),
                "{name}: an inner proxy's line follows that",
            );
        }
    }

    /// A proxy on one line, the shape nearly every request has, costs no copy.
    #[test]
    fn a_list_on_one_field_line_is_read_in_place() {
        let mut headers = HeaderMap::new();
        headers.append(
            X_FORWARDED_FOR,
            http::HeaderValue::from_static("203.0.113.7"),
        );
        assert!(matches!(
            field_list(&headers, X_FORWARDED_FOR),
            Some(Cow::Borrowed("203.0.113.7")),
        ));
        assert_eq!(field_list(&headers, header::FORWARDED), None);
    }

    /// Bytes no text holds, on a caller's own line or beside a hop, are no node:
    /// the list they sit in names no client.
    #[test]
    fn a_line_or_a_hop_no_text_holds_names_no_client() {
        for (name, forged, own, same_line) in [
            (
                "x-forwarded-for",
                &b"\xFF"[..],
                &b"203.0.113.7"[..],
                &b"\xFF, 203.0.113.7"[..],
            ),
            (
                "forwarded",
                &b"for=\xFF"[..],
                &b"for=203.0.113.7"[..],
                &b"for=\xFF, for=203.0.113.7"[..],
            ),
        ] {
            for lines in [&[(name, forged), (name, own)][..], &[(name, same_line)]] {
                refused_whole(lines, name);
            }
        }
    }

    /// A quote no quote closes reads two ways once a proxy writes after it, on a
    /// line of its own or appended to the caller's, quoted or not: no chain.
    #[test]
    fn a_caller_s_unclosed_quote_leaves_no_chain() {
        for own in [&b"for=203.0.113.7"[..], &b"for=\"203.0.113.7:4711\""[..]] {
            refused_whole(
                &[("forwarded", b"for=198.51.100.99;x=\""), ("forwarded", own)],
                "forwarded",
            );
        }
        for raw in [
            "for=198.51.100.99;x=\", for=203.0.113.7",
            "for=198.51.100.99;x=\"a\\\", for=203.0.113.7",
            "for=198.51.100.99;x=\", for=\"203.0.113.7:4711\"",
            "for=198.51.100.99;x=\", for=203.0.113.7;host=\"a,for=198.51.100.98\"",
        ] {
            refused_whole(&[("forwarded", raw.as_bytes())], "forwarded");
        }
    }

    /// `X-Real-IP` names one hop, and a proxy adding its line adds it last.
    #[test]
    fn a_real_ip_over_several_field_lines_is_its_last() {
        assert_eq!(
            from_the_proxy(&[
                ("x-real-ip", b"198.51.100.99"),
                ("x-real-ip", b"203.0.113.7"),
            ]),
            ClientOrigin::Forwarded(ip("203.0.113.7")),
        );
    }

    /// The same two lines from a peer off the list are the caller's to write.
    #[test]
    fn an_untrusted_peer_ignores_every_field_line() {
        let req = req_from(
            "203.0.113.200",
            &[
                ("x-forwarded-for", "198.51.100.99"),
                ("x-forwarded-for", "203.0.113.7"),
            ],
        );
        assert_eq!(
            ClientOrigin::of_with(&req, &listed(&[ip("10.0.0.1")])).0,
            ClientOrigin::Peer(ip("203.0.113.200")),
        );
    }

    /// An ingress whose pods move holds no address list: its range names every
    /// pod, the peer and each hop of the chain alike.
    #[test]
    fn a_peer_inside_a_listed_range_is_trusted() {
        let trusted = trusting(&["10.0.0.0/8", "fd00::/8"]).trusted_proxies;
        let of = |xff, peer: &str| {
            ClientOrigin::resolve(None, Some(xff), None, Some(ip(peer)), &trusted)
        };
        let client = ClientOrigin::Forwarded(ip("203.0.113.50"));
        assert_eq!(of("203.0.113.50", "10.42.7.9"), client);
        assert_eq!(of("203.0.113.50", "::ffff:10.42.7.9"), client);
        assert_eq!(of("203.0.113.50", "fd12::9"), client);
        assert_eq!(of("203.0.113.50, 10.42.0.1", "10.42.7.9"), client);
        assert_eq!(of("203.0.113.50, fd12::1", "10.42.7.9"), client);
        assert_eq!(
            of("203.0.113.50", "11.0.0.1"),
            ClientOrigin::Peer(ip("11.0.0.1")),
            "a peer past the range forges nothing",
        );
    }

    /// A dual-stack listener reports an IPv4 proxy as `::ffff:a.b.c.d`: it is
    /// the proxy its IPv4 entry names, or every caller reads as the proxy.
    #[test]
    fn a_mapped_peer_matches_its_ipv4_entry() {
        let origin = resolve(
            Some("203.0.113.50"),
            Some(ip("::ffff:10.0.0.1")),
            &[ip("10.0.0.1")],
        );
        assert_eq!(origin, ClientOrigin::Forwarded(ip("203.0.113.50")));
    }

    /// Off any proxy a client is its IPv4 address, in the spelling a caller
    /// compares and a bucket keys on, never the mapped one only the socket uses.
    #[test]
    fn a_mapped_peer_is_attributed_to_its_ipv4_address() {
        assert_eq!(
            resolve(None, Some(ip("::ffff:203.0.113.50")), &[]),
            ClientOrigin::Peer(ip("203.0.113.50")),
        );
    }

    #[test]
    fn a_mapped_proxy_forwarding_nothing_is_reported_as_its_ipv4_address() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            resolve(None, Some(ip("::ffff:10.0.0.1")), &[proxy]),
            ClientOrigin::TrustedProxy(proxy),
        );
    }

    /// A proxy records the peer it read, so a chain may hold the mapped form:
    /// it names the same proxy, and the same client.
    #[test]
    fn a_mapped_hop_is_the_ipv4_address_it_carries() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            resolve(
                Some("203.0.113.50, ::ffff:10.0.0.2"),
                Some(proxy),
                &[proxy, ip("10.0.0.2")]
            ),
            ClientOrigin::Forwarded(ip("203.0.113.50")),
            "a proxy's own hop in the chain",
        );
        assert_eq!(
            resolve(Some("::ffff:203.0.113.50"), Some(proxy), &[proxy]),
            ClientOrigin::Forwarded(ip("203.0.113.50")),
            "the client's hop",
        );
    }

    #[test]
    fn a_real_ip_naming_a_mapped_proxy_names_no_client() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            ClientOrigin::resolve(
                None,
                None,
                Some("::ffff:10.0.0.1"),
                Some(proxy),
                &listed(&[proxy]),
            ),
            ClientOrigin::TrustedProxy(proxy),
        );
    }

    /// Two proxies spelling one client two ways do not disagree about it.
    #[test]
    fn two_chains_naming_one_client_in_two_spellings_agree() {
        let proxy = ip("10.0.0.1");
        assert_eq!(
            ClientOrigin::resolve(
                Some("for=\"[::ffff:203.0.113.50]\""),
                Some("203.0.113.50"),
                None,
                Some(proxy),
                &listed(&[proxy]),
            ),
            ClientOrigin::Forwarded(ip("203.0.113.50")),
        );
    }

    const UNREADABLE: &str = "forwarding header unreadable — no client is believed";
    const DISAGREEMENT: &str = "forwarding headers disagree about the client — neither is believed";

    /// A listed peer's head that names no client, the peer standing, and the one
    /// `warn` it filed naming `header`, which quotes none of the head's hops.
    #[track_caller]
    fn refused_whole(lines: &[(&str, &[u8])], header: &str) {
        let logs = nest_rs_testing::LogCapture::install();
        assert_eq!(
            from_the_proxy(lines),
            ClientOrigin::TrustedProxy(ip("10.0.0.1")),
            "{lines:?}",
        );
        let filed = logs.expect_one(crate::target::HTTP, UNREADABLE);
        assert_eq!(filed.level, "warn");
        assert_eq!(filed.field("header").as_deref(), Some(header), "{lines:?}");
        assert_eq!(filed.field("peer").as_deref(), Some("10.0.0.1"));
        for hop in ["198.51.100.99", "203.0.113.7", "garbage"] {
            assert!(!logs.quotes(hop), "{:?}", filed.fields);
        }
    }

    /// A proxy appending to the caller's own line a quoted value that starts with
    /// a separator closes the caller's quote there: the bytes read two ways, so
    /// the chain is no chain and the caller's hop is not the client.
    #[test]
    fn a_quote_a_proxy_appended_to_the_caller_s_line_chooses_no_client() {
        let line = b"for=198.51.100.99;x=\", for=203.0.113.7;host=\",a\"";
        refused_whole(&[("forwarded", line)], "forwarded");
    }

    /// Every way a `Forwarded` value fails its grammar reads the same: no chain.
    #[test]
    fn a_forwarded_value_its_grammar_refuses_is_no_chain() {
        for line in [
            &b"for=198.51.100.99;x=\""[..],
            b"for=\"203.0.113.7\"x",
            b"for=203.0.113.7\"",
            b"for=198.51.100.99, for",
            b"=203.0.113.7",
            b"for=",
            b"for=garbage",
            b"for=\"[203.0.113.7]\"",
            b"for=\xFF",
            b"for=203.0.113.7 198.51.100.99",
        ] {
            refused_whole(&[("forwarded", line)], "forwarded");
        }
    }

    /// A de-facto list is read whole too: an element that is no node leaves the
    /// hop beside it standing for nobody.
    #[test]
    fn an_x_forwarded_for_element_that_is_no_node_is_no_chain() {
        for line in [
            &b"198.51.100.99, garbage"[..],
            b"garbage, 203.0.113.7",
            b"\xFF, 203.0.113.7",
            b"203.0.113.7:99999",
            b"[malformed::]",
        ] {
            refused_whole(&[("x-forwarded-for", line)], "x-forwarded-for");
        }
    }

    #[test]
    fn a_real_ip_that_is_no_node_is_no_hop() {
        for line in [&b"garbage"[..], b"203.0.113.7, 198.51.100.99", b"\xFF"] {
            refused_whole(&[("x-real-ip", line)], "x-real-ip");
        }
    }

    /// A chain nobody can read decides for every header: ignored alone, it would
    /// hand the answer to the one the caller wrote, behind a proxy appending to
    /// `Forwarded` and passing `X-Forwarded-For` through.
    #[test]
    fn an_unreadable_chain_hands_the_answer_to_no_other_header() {
        refused_whole(
            &[
                ("forwarded", b"x=\", for=203.0.113.7;host=\",a\""),
                ("x-forwarded-for", b"198.51.100.99"),
                ("x-real-ip", b"198.51.100.99"),
            ],
            "forwarded",
        );
        refused_whole(
            &[
                ("x-forwarded-for", b"203.0.113.7, garbage"),
                ("x-real-ip", b"198.51.100.99"),
            ],
            "x-forwarded-for",
        );
    }

    /// The hop facing the client withheld it (RFC 7239 §6.2, §6.3): the hop to
    /// its left is the caller's own, so no client is named, on either chain.
    #[test]
    fn a_client_the_proxy_withheld_hands_the_answer_to_no_hop_left_of_it() {
        let proxy = ip("10.0.0.1");
        for (forwarded, forwarded_for) in [
            (Some("for=198.51.100.99, for=unknown"), None),
            (Some("for=198.51.100.99, for=_hidden"), None),
            (Some("for=198.51.100.99, for=\"_hidden:_port\""), None),
            (None, Some("198.51.100.99, unknown")),
            (Some("for=unknown"), Some("198.51.100.99")),
            (Some("for=198.51.100.99"), Some("unknown")),
        ] {
            let logs = nest_rs_testing::LogCapture::install();
            assert_eq!(
                ClientOrigin::resolve(
                    forwarded,
                    forwarded_for,
                    Some("198.51.100.99"),
                    Some(proxy),
                    &listed(&[proxy])
                ),
                ClientOrigin::TrustedProxy(proxy),
                "{forwarded:?} {forwarded_for:?}",
            );
            assert!(
                logs.events_at(tracing::Level::WARN).is_empty(),
                "a node RFC 7239 names is no fault"
            );
        }
    }

    /// `X-Real-IP` beside a chain naming the client is read whole and must name
    /// that client too: behind a proxy that writes `X-Real-IP` and passes the
    /// caller's chain through, the chain is the caller's, so another client there
    /// is a disagreement, refused as two chains are. A hop of the proxies' own
    /// names no client, and changes nothing.
    #[test]
    fn a_real_ip_beside_a_chain_names_its_client_or_none_is_believed() {
        let proxy = ip("10.0.0.1");
        let client = ClientOrigin::Forwarded(ip("203.0.113.7"));
        let refused = ClientOrigin::TrustedProxy(proxy);
        for (forwarded, forwarded_for) in
            [(Some("for=203.0.113.7"), None), (None, Some("203.0.113.7"))]
        {
            for (real_ip, answer, filed) in [
                ("203.0.113.7", client, None),
                ("::ffff:203.0.113.7", client, None),
                ("10.0.0.1", client, None),
                ("unknown", refused, None),
                ("198.51.100.99", refused, Some(DISAGREEMENT)),
                ("garbage", refused, Some(UNREADABLE)),
                ("\u{FFFD}", refused, Some(UNREADABLE)),
            ] {
                let case = format!("{forwarded:?} {forwarded_for:?} {real_ip:?}");
                let logs = nest_rs_testing::LogCapture::install();
                assert_eq!(
                    ClientOrigin::resolve(
                        forwarded,
                        forwarded_for,
                        Some(real_ip),
                        Some(proxy),
                        &listed(&[proxy]),
                    ),
                    answer,
                    "{case}",
                );
                let Some(message) = filed else {
                    let warned = logs.events_at(tracing::Level::WARN);
                    assert!(warned.is_empty(), "{case}: {warned:?}");
                    continue;
                };
                let line = logs.expect_one(crate::target::HTTP, message);
                assert_eq!(line.field("peer").as_deref(), Some("10.0.0.1"), "{case}");
                if message == UNREADABLE {
                    assert_eq!(line.field("header").as_deref(), Some(X_REAL_IP), "{case}");
                }
                for hop in ["198.51.100.99", "203.0.113.7", "garbage"] {
                    assert!(!logs.quotes(hop), "{case}: {:?}", line.fields);
                }
            }
        }
    }

    /// A request carrying a real transport peer.
    fn req_from(peer: &str, headers: &[(&str, &str)]) -> Request {
        let socket: SocketAddr = format!("{peer}:54321").parse().expect("test literal");
        let mut builder = Request::builder().peer_addr(socket);
        for (name, value) in headers {
            builder = builder.header(
                http::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                http::HeaderValue::from_str(value).expect("header value"),
            );
        }
        builder.finish()
    }

    /// Extract under an ambient request scope over a container holding `config`.
    async fn extract_under(config: Option<HttpConfig>, req: Request) -> ClientIp {
        let mut builder = nest_rs_core::Container::builder();
        if let Some(config) = config {
            builder = builder.provide(config);
        }
        let scope = std::sync::Arc::new(nest_rs_core::RequestScope::new(
            builder.build().expect("the container builds"),
        ));
        nest_rs_core::with_request_scope(
            Some(scope),
            nest_rs_core::Correlation::minted(None),
            extract(req),
        )
        .await
    }

    fn trusting(proxies: &[&str]) -> HttpConfig {
        HttpConfig {
            trusted_proxies: proxies
                .iter()
                .map(|p| p.parse().expect("an entry"))
                .collect(),
            ..HttpConfig::default()
        }
    }

    #[tokio::test]
    async fn the_extractor_reads_the_trusted_proxies_off_the_booted_config() {
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        let extracted = extract_under(Some(trusting(&["10.0.0.1"])), req).await;
        assert_eq!(extracted.ip, ip("203.0.113.50"));
        assert!(extracted.forwarded);
    }

    #[tokio::test]
    async fn the_extractor_reads_the_rfc_7239_header() {
        let req = req_from(
            "10.0.0.1",
            &[("forwarded", "for=192.0.2.60;proto=http;by=203.0.113.43")],
        );
        let extracted = extract_under(Some(trusting(&["10.0.0.1"])), req).await;
        assert_eq!(extracted.ip, ip("192.0.2.60"));
        assert!(extracted.forwarded);
    }

    #[tokio::test]
    async fn an_empty_trusted_proxy_list_resolves_the_same_request_to_the_peer() {
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        let extracted = extract_under(Some(HttpConfig::default()), req).await;
        assert_eq!(extracted.ip, ip("10.0.0.1"));
        assert!(!extracted.forwarded);
    }

    #[tokio::test]
    async fn no_reachable_config_trusts_nothing() {
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        assert_eq!(extract_under(None, req).await.ip, ip("10.0.0.1"));
        let req = req_from("10.0.0.1", &[("x-forwarded-for", "203.0.113.50")]);
        assert_eq!(extract(req).await.ip, ip("10.0.0.1"), "and off any scope");
    }

    async fn extract(req: Request) -> ClientIp {
        let (req, mut body) = req.split();
        <ClientIp as crate::FromRequest>::from_request(&req, &mut body)
            .await
            .expect("the extractor is infallible")
    }

    #[tokio::test]
    async fn a_request_with_no_peer_and_no_trusted_proxy_extracts_the_default() {
        // A built `Request` has no peer socket.
        let ip = extract(Request::builder().finish()).await;
        assert_eq!(ip, ClientIp::unknown());
    }

    #[tokio::test]
    async fn headers_alone_never_set_forwarded() {
        let req = Request::builder()
            .header(
                http::HeaderName::from_static("x-forwarded-for"),
                http::HeaderValue::from_static("9.9.9.9"),
            )
            .header(
                http::HeaderName::from_static("x-real-ip"),
                http::HeaderValue::from_static("9.9.9.9"),
            )
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
