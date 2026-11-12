//! [`OriginPolicy`] — whether a browser's `Origin` may open an upgrade or an
//! MCP exchange, the requests a CORS policy cannot protect: a browser consults
//! CORS before it lets a page read a response, never before it opens a socket
//! or sends a request whose effect is the point.
//!
//! The policy is the transport's own: the origins its CORS policy lists,
//! parsed once at boot into [`ListedOrigins`], which the CORS layer reads too,
//! so one entry answers alike on both; and the proxies its configuration
//! trusts. With CORS configured, the transport's CORS layer answers first and
//! admits only what it lists, the server's own origin included; an entry point
//! mounted outside the transport is held to this policy all the same.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use http::HeaderValue;
use http::header::{self, HeaderMap};
use http::uri::Authority;

use crate::client_ip::forwarded_param;
use crate::cors::{WILDCARD, holds_wildcard};
use crate::endpoint::{BoxEndpoint, Endpoint, OriginCheck, OriginGate};
use crate::error::{NotAnOrigin, OriginRefused, OriginRefusedReason, ResponseError};
use crate::trusted_proxy::{TrustedProxy, lists};

/// The de-facto forwarding headers for the scheme and the host, which the
/// `http` crate does not name.
const X_FORWARDED_PROTO: &str = "x-forwarded-proto";
const X_FORWARDED_HOST: &str = "x-forwarded-host";

/// Whether a browser `Origin` may open an upgrade or an MCP exchange: absent ⇒
/// yes, no browser sent it; present ⇒ it is the request's own origin, or the
/// CORS policy lists it (`*` lists every origin — an explicit opening).
///
/// "The request's own origin" is `scheme://host[:port]`: the scheme the
/// request arrived over and its `Host` — or, from a trusted proxy alone, what
/// `Forwarded` (`proto=`, `host=`) or `X-Forwarded-Proto` / `X-Forwarded-Host`
/// say the client asked for. Origins compare per RFC 6454 §5: scheme and host
/// without case, a non-special scheme's host byte for byte, ports with the
/// scheme's default filled in.
#[derive(Clone, Debug)]
pub(crate) struct OriginPolicy {
    /// What the CORS policy lists; `None` without one.
    listed: Option<ListedOrigins>,
    trusted_proxies: Box<[TrustedProxy]>,
}

impl OriginPolicy {
    /// The policy of a transport whose CORS policy lists `listed`, `None`
    /// without one, behind the proxies `trusted_proxies` names.
    pub(crate) fn new(listed: Option<ListedOrigins>, trusted_proxies: Box<[TrustedProxy]>) -> Self {
        Self {
            listed,
            trusted_proxies,
        }
    }

    /// `Ok` when `request` carries no `Origin`, its own, or one the CORS policy
    /// lists; otherwise the refusal, which answers `403`. A second `Origin`
    /// field, or one that is no origin serialization, is refused.
    pub(crate) fn allows(&self, request: &crate::Request) -> Result<(), OriginRefused> {
        let mut sent = request.headers().get_all(header::ORIGIN).iter();
        let Some(origin) = sent.next() else {
            return Ok(());
        };
        let trusted_proxy = request
            .peer_addr()
            .is_some_and(|peer| lists(&self.trusted_proxies, peer.ip()));
        let refused = |reason, scheme| Err(OriginRefused::new(reason, scheme, trusted_proxy));
        if sent.next().is_some() {
            return refused(OriginRefusedReason::OriginRepeated, None);
        }
        let Ok(origin) = origin.to_str() else {
            return refused(OriginRefusedReason::OriginNotVisibleAscii, None);
        };
        let Some(origin) = Origin::parse(origin) else {
            return refused(OriginRefusedReason::OriginNotSerialized, None);
        };
        if self
            .listed
            .as_ref()
            .is_some_and(|listed| listed.lists(&origin))
        {
            return Ok(());
        }
        let Origin::Tuple(sent) = origin else {
            return refused(OriginRefusedReason::OriginNull, None);
        };
        match self.own(request, trusted_proxy) {
            Some(own) if own == sent => Ok(()),
            own => refused(
                own.map_or(OriginRefusedReason::HostMissing, |own| {
                    sent.differs_from(&own)
                }),
                Some(OriginScheme::of(&sent.scheme)),
            ),
        }
    }

    /// The origin the request was sent to: its `Host` and the scheme it
    /// arrived over, or what a trusted proxy forwarded of them.
    fn own(&self, request: &crate::Request, proxied: bool) -> Option<Tuple> {
        let headers = request.headers();
        let (proto, host) = if proxied {
            forwarded(headers)
        } else {
            (None, None)
        };
        let scheme = proto.unwrap_or_else(|| request.scheme().as_str());
        let host = host
            .or_else(|| headers.get(header::HOST)?.to_str().ok())
            .or_else(|| request.original_uri().authority().map(Authority::as_str))?;
        Tuple::of(scheme, host).ok()
    }
}

/// The origins a CORS policy lists, parsed once at boot: the one list the
/// CORS layer and the browser-origin check both read, so an entry answers
/// alike on both.
#[derive(Clone, Debug)]
pub(crate) enum ListedOrigins {
    /// Each origin, listed once.
    Origins(Arc<[ListedOrigin]>),
    /// `*`.
    Every,
}

/// One listed origin: the tuple two origins compare by, and the serialization
/// a browser sends for it.
#[derive(Debug)]
pub(crate) struct ListedOrigin {
    tuple: Tuple,
    serialized: Box<str>,
}

impl ListedOrigins {
    /// `entries` read as origins, `*` listing every one; or the first entry
    /// that is none, by its place from zero and why. Each entry must be what
    /// a browser could send: `null` is refused, since any sandboxed or
    /// `data:` document sends it whatever site it runs on.
    pub(crate) fn parse(entries: &[String]) -> Result<Self, (usize, NotAnOrigin)> {
        let mut listed: Vec<ListedOrigin> = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry == WILDCARD {
                continue;
            }
            if entry.eq_ignore_ascii_case("null") {
                return Err((index, NotAnOrigin::Null));
            }
            let tuple = Tuple::parse(entry).map_err(|defect| (index, defect))?;
            if !listed.iter().any(|origin| origin.tuple == tuple) {
                let serialized = tuple.serialization().into_boxed_str();
                listed.push(ListedOrigin { tuple, serialized });
            }
        }
        Ok(match holds_wildcard(entries) {
            true => Self::Every,
            false => Self::Origins(listed.into()),
        })
    }

    /// Whether the CORS layer admits a request whose `Origin` is `sent`: its
    /// origin is listed, compared as [`OriginPolicy`] compares it.
    pub(crate) fn admits(&self, sent: &HeaderValue) -> bool {
        match self {
            Self::Every => true,
            // A browser sends the serialization each entry keeps, so the bytes
            // decide what a browser sends before any parse.
            Self::Origins(listed) => {
                listed
                    .iter()
                    .any(|origin| origin.serialized.as_bytes() == sent.as_bytes())
                    || sent
                        .to_str()
                        .ok()
                        .and_then(Origin::parse)
                        .is_some_and(|origin| self.lists(&origin))
            }
        }
    }

    /// Whether `origin` is listed: an opaque one never is, but by `*`.
    fn lists(&self, origin: &Origin) -> bool {
        match (self, origin) {
            (Self::Every, _) => true,
            (Self::Origins(listed), Origin::Tuple(sent)) => {
                listed.iter().any(|origin| origin.tuple == *sent)
            }
            (Self::Origins(_), Origin::Opaque) => false,
        }
    }
}

/// What a trusted proxy says the client asked for: `Forwarded`'s `proto` and
/// `host` (RFC 7239 §5.3, §5.4), else `X-Forwarded-Proto` and
/// `X-Forwarded-Host` — each the first value, which the hop facing the client
/// wrote. A value a caller prepended can only claim an origin, and a caller
/// that is no browser may send none at all.
fn forwarded(headers: &HeaderMap) -> (Option<&str>, Option<&str>) {
    (
        forwarded_proto(headers),
        forwarded_named(headers, "host", X_FORWARDED_HOST),
    )
}

/// The scheme a trusted proxy says the client asked for, read as
/// [`forwarded`] reads it: `Forwarded`'s `proto`, else `X-Forwarded-Proto`.
pub(crate) fn forwarded_proto(headers: &HeaderMap) -> Option<&str> {
    forwarded_named(headers, "proto", X_FORWARDED_PROTO)
}

/// `Forwarded`'s `param`, else the first value of the de-facto `header`.
fn forwarded_named<'a>(headers: &'a HeaderMap, param: &str, header: &str) -> Option<&'a str> {
    headers
        .get(header::FORWARDED)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| forwarded_param(raw, param))
        .or_else(|| {
            headers
                .get(header)?
                .to_str()
                .ok()?
                .split(',')
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
}

/// An entry behind its surface's origin check, run before any guard: a refused
/// origin never pays for authentication.
pub(crate) struct OriginChecked {
    policy: OriginPolicy,
    gate: OriginGate,
    inner: BoxEndpoint,
}

impl OriginChecked {
    /// `inner`, the requests `gate` names checked against `policy`.
    pub(crate) fn new(policy: OriginPolicy, gate: OriginGate, inner: BoxEndpoint) -> Self {
        Self {
            policy,
            gate,
            inner,
        }
    }
}

impl Endpoint for OriginChecked {
    async fn call(&self, req: crate::Request) -> crate::Result<crate::Response> {
        let checked = match self.gate.on {
            OriginCheck::Every => true,
            OriginCheck::Upgrades => crate::websocket::requested(req.headers()),
        };
        if checked && let Err(refused) = self.policy.allows(&req) {
            (self.gate.refused)(&refused);
            return Ok(refused.as_response());
        }
        self.inner.call(req).await
    }
}

/// An origin (RFC 6454 §4), in the form two compare in (§5).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Origin {
    /// `null`, an opaque origin (§7.3): admitted only where `*` is listed.
    Opaque,
    /// A scheme, a host and a port.
    Tuple(Tuple),
}

/// Scheme lowercased, host as [`canonical_host`] reads it, and the port the
/// scheme defaults to when none is written.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Tuple {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl Origin {
    /// An `Origin` serialization (§6.2): `null`, or `scheme "://" host [ ":"
    /// port ]` with no path.
    fn parse(serialized: &str) -> Option<Self> {
        if serialized == "null" {
            return Some(Self::Opaque);
        }
        Tuple::parse(serialized).ok().map(Self::Tuple)
    }
}

impl Tuple {
    /// `scheme "://" host [ ":" port ]`, or the first way `serialized` is not
    /// one.
    fn parse(serialized: &str) -> Result<Self, NotAnOrigin> {
        if !serialized.is_ascii() {
            return Err(NotAnOrigin::NonAscii);
        }
        if serialized
            .bytes()
            .any(|b| b.is_ascii_control() || b == b' ')
        {
            return Err(NotAnOrigin::Character);
        }
        let (scheme, rest) = serialized.split_once("://").ok_or(NotAnOrigin::NoScheme)?;
        if let Some(delimiter) = rest.bytes().find(|b| matches!(b, b'/' | b'?' | b'#')) {
            return Err(match delimiter {
                b'/' => NotAnOrigin::Path,
                b'?' => NotAnOrigin::Query,
                _ => NotAnOrigin::Fragment,
            });
        }
        Self::of(scheme, rest)
    }

    /// The origin of a request sent to `authority` over `scheme`.
    fn of(scheme: &str, authority: &str) -> Result<Self, NotAnOrigin> {
        let scheme_is_valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
        if !scheme_is_valid {
            return Err(NotAnOrigin::Scheme);
        }
        // Userinfo is no part of an origin, nor of a `Host`.
        if authority.contains('@') {
            return Err(NotAnOrigin::Userinfo);
        }
        if authority.is_empty() {
            return Err(NotAnOrigin::NoHost);
        }
        // A browser decodes a percent-encoded host before it serializes it.
        if authority.contains('%') {
            return Err(NotAnOrigin::Host);
        }
        let parsed: Authority = authority.parse().ok().ok_or(NotAnOrigin::Host)?;
        let host = parsed.host();
        let port = match authority.get(host.len()..) {
            Some("" | ":") => None,
            Some(written) => Some(
                written
                    .strip_prefix(':')
                    .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|digits| digits.parse::<u16>().ok())
                    .ok_or(NotAnOrigin::Port)?,
            ),
            None => return Err(NotAnOrigin::Host),
        };
        let scheme = scheme.to_ascii_lowercase();
        let host = canonical_host(&scheme, host)?;
        let port = port.or_else(|| default_port(&scheme));
        Ok(Self { scheme, host, port })
    }

    /// The serialization a browser sends for this origin (§6.2): the port
    /// written only when it is not the scheme's default.
    fn serialization(&self) -> String {
        match self
            .port
            .filter(|&port| Some(port) != default_port(&self.scheme))
        {
            Some(port) => format!("{}://{}:{port}", self.scheme, self.host),
            None => format!("{}://{}", self.scheme, self.host),
        }
    }

    /// The first component, in §5's order, in which `self` is not `own`.
    fn differs_from(&self, own: &Self) -> OriginRefusedReason {
        if self.scheme != own.scheme {
            OriginRefusedReason::SchemeDiffers
        } else if self.host != own.host {
            OriginRefusedReason::HostDiffers
        } else {
            OriginRefusedReason::PortDiffers
        }
    }
}

/// A refused origin's scheme, from a closed set: the two a page is served
/// over, and any other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OriginScheme {
    /// `http`.
    Http,
    /// `https`.
    Https,
    /// Any other scheme.
    Other,
}

impl OriginScheme {
    /// The scheme of a lowercased origin.
    pub(crate) fn of(scheme: &str) -> Self {
        match scheme {
            "http" => Self::Http,
            "https" => Self::Https,
            _ => Self::Other,
        }
    }

    /// The word the line files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Other => "other",
        }
    }
}

/// `host` as two origins over the lowercased `scheme` compare it, as the URL
/// standard's *host parser* reads it: an IPv6 literal in its canonical text;
/// over a special scheme, lowercase, a host that ends in a number the IPv4
/// address it reads as (`127.1` is `127.0.0.1`); over any other, opaque, the
/// bytes written. Refused when it is empty, a malformed literal, or, over a
/// special scheme, ends in a number and is no IPv4 address.
fn canonical_host(scheme: &str, host: &str) -> Result<String, NotAnOrigin> {
    if let Some(literal) = host.strip_prefix('[') {
        let address: Ipv6Addr = literal
            .strip_suffix(']')
            .and_then(|literal| literal.parse().ok())
            .ok_or(NotAnOrigin::Host)?;
        return Ok(format!("[{address}]"));
    }
    if host.is_empty() {
        return Err(NotAnOrigin::NoHost);
    }
    if !is_special(scheme) {
        return Ok(host.to_owned());
    }
    let host = host.to_ascii_lowercase();
    if !ends_in_a_number(&host) {
        return Ok(host);
    }
    ipv4(&host)
        .map(|address| address.to_string())
        .ok_or(NotAnOrigin::Host)
}

/// Whether the URL standard calls the lowercased `scheme` special (WHATWG URL,
/// *special scheme*).
fn is_special(scheme: &str) -> bool {
    matches!(scheme, "ftp" | "file" | "http" | "https" | "ws" | "wss")
}

/// Whether the URL standard reads `host` as an IPv4 address (WHATWG URL,
/// *ends in a number checker*): its last label, one trailing `.` aside, is
/// decimal digits or an IPv4 number.
fn ends_in_a_number(host: &str) -> bool {
    let host = host.trim_suffix('.');
    let last = host.rsplit_once('.').map_or(host, |(_, last)| last);
    (!last.is_empty() && last.bytes().all(|b| b.is_ascii_digit())) || ipv4_number(last).is_some()
}

/// `host` as the URL standard's *IPv4 parser* reads it: at most four parts,
/// one trailing `.` aside, each but the last a byte and the last filling the
/// bytes left; `None` when it is no IPv4 address.
fn ipv4(host: &str) -> Option<Ipv4Addr> {
    let host = host.trim_suffix('.');
    let numbers = host
        .split('.')
        .map(ipv4_number)
        .collect::<Option<Vec<u64>>>()?;
    let (&last, leading) = numbers.split_last()?;
    if leading.len() > 3 || leading.iter().any(|&byte| byte > 255) {
        return None;
    }
    if last >> (8 * (4 - leading.len())) != 0 {
        return None;
    }
    let address = leading
        .iter()
        .zip([24, 16, 8])
        .fold(last, |address, (&byte, shift)| address + (byte << shift));
    u32::try_from(address).ok().map(Ipv4Addr::from)
}

/// One part of an IPv4 address as the URL standard's *IPv4 number parser*
/// reads it: hexadecimal after `0x`, octal after a leading `0`, else decimal,
/// a bare prefix reading `0`; `None` when it is no number. A value past `u64`
/// saturates, too large for any address all the same.
fn ipv4_number(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    let (digits, radix) = if let Some(hex) = part.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(octal) = part.strip_prefix('0').filter(|octal| !octal.is_empty()) {
        (octal, 8)
    } else {
        (part, 10)
    };
    digits.chars().try_fold(0_u64, |value, digit| {
        let digit = digit.to_digit(radix)?;
        Some(
            value
                .saturating_mul(u64::from(radix))
                .saturating_add(u64::from(digit)),
        )
    })
}

/// The port a scheme's URLs default to (RFC 9110 §4.2, RFC 6455 §3).
fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use http::{HeaderValue, Uri};

    use super::*;
    use crate::Request;
    use crate::error::{ORIGIN_REFUSED, ResponseError};

    const PROXY: &str = "10.0.0.1";

    fn entries(origins: &[&str]) -> Vec<String> {
        origins.iter().map(|&origin| origin.to_owned()).collect()
    }

    /// The policy of a transport whose CORS policy lists `origins`, none
    /// meaning no CORS policy, behind the proxies `trusted` names.
    fn policy(origins: &[&str], trusted: &[&str]) -> OriginPolicy {
        let listed = (!origins.is_empty())
            .then(|| ListedOrigins::parse(&entries(origins)).expect("the test's list parses"));
        OriginPolicy::new(
            listed,
            trusted
                .iter()
                .map(|entry| entry.parse().expect("a test literal is an entry"))
                .collect(),
        )
    }

    /// Each entry no browser sends as an origin is refused at its place, by
    /// the first way it is not one.
    #[test]
    fn an_entry_that_is_no_origin_is_refused_by_kind() {
        for (entry, kind) in [
            ("null", NotAnOrigin::Null),
            ("Null", NotAnOrigin::Null),
            ("https://é.example", NotAnOrigin::NonAscii),
            ("https://app\u{7f}.example", NotAnOrigin::Character),
            ("https://app .example", NotAnOrigin::Character),
            ("app.example", NotAnOrigin::NoScheme),
            ("1https://app.example", NotAnOrigin::Scheme),
            ("https://user@app.example", NotAnOrigin::Userinfo),
            ("https://app.example/", NotAnOrigin::Path),
            ("https://app.example/path", NotAnOrigin::Path),
            ("https://app.example?query", NotAnOrigin::Query),
            ("https://app.example#fragment", NotAnOrigin::Fragment),
            ("https://", NotAnOrigin::NoHost),
            ("https://:443", NotAnOrigin::NoHost),
            ("https://app%2Eexample", NotAnOrigin::Host),
            ("https://[::1", NotAnOrigin::Host),
            ("https://[nope]", NotAnOrigin::Host),
            ("https://app.123", NotAnOrigin::Host),
            ("https://app.0x1", NotAnOrigin::Host),
            ("https://1.2.3.08", NotAnOrigin::Host),
            ("https://1..2", NotAnOrigin::Host),
            ("https://256.0.0.1", NotAnOrigin::Host),
            ("https://1.2.3.256", NotAnOrigin::Host),
            ("https://1.2.3.4.5", NotAnOrigin::Host),
            ("https://4294967296", NotAnOrigin::Host),
            ("https://app.example:99999", NotAnOrigin::Port),
            ("https://app.example:https", NotAnOrigin::Port),
            ("https://app.example:+443", NotAnOrigin::Port),
        ] {
            let refused = ListedOrigins::parse(&entries(&["https://listed.example", entry]))
                .map(|_| ())
                .expect_err(entry);
            assert_eq!(refused, (1, kind), "{entry:?}");
        }
    }

    /// An entry is the origin it parses to: written without case or with its
    /// default port, it is listed once, and kept as a browser serializes it.
    #[test]
    fn an_entry_is_kept_as_the_origin_it_parses_to() {
        let ListedOrigins::Origins(listed) = ListedOrigins::parse(&entries(&[
            "HTTPS://App.Example:443",
            "https://app.example",
            "http://[0:0:0:0:0:0:0:1]:8080",
            "chrome-extension://abcdef",
        ]))
        .expect("the list parses") else {
            panic!("no wildcard is listed");
        };
        let serialized: Vec<&str> = listed.iter().map(|origin| &*origin.serialized).collect();
        assert_eq!(
            serialized,
            [
                "https://app.example",
                "http://[::1]:8080",
                "chrome-extension://abcdef"
            ]
        );
        for origin in listed.iter() {
            assert_eq!(
                Origin::parse(&origin.serialized),
                Some(Origin::Tuple(origin.tuple.clone())),
                "{}",
                origin.serialized,
            );
        }
    }

    /// A host that ends in a number is the IPv4 address the URL standard
    /// reads it as, which is what a browser on it sends; a host whose last
    /// label is no number stays a domain name.
    #[test]
    fn a_host_that_ends_in_a_number_is_the_ipv4_address_it_reads_as() {
        for (entry, origin) in [
            ("https://127.1", "https://127.0.0.1"),
            ("https://0x7f.0.0.1", "https://127.0.0.1"),
            ("https://0X7F.1", "https://127.0.0.1"),
            ("https://0177.0.0.1", "https://127.0.0.1"),
            ("https://2130706433", "https://127.0.0.1"),
            ("https://127.0.0.1.", "https://127.0.0.1"),
            ("https://1.2.3.04", "https://1.2.3.4"),
            ("https://0x", "https://0.0.0.0"),
            ("https://4294967295", "https://255.255.255.255"),
            ("http://127.1:8080", "http://127.0.0.1:8080"),
            ("https://1.2.3.app", "https://1.2.3.app"),
            ("https://app.1x", "https://app.1x"),
        ] {
            assert_eq!(
                Tuple::parse(entry).map(|tuple| tuple.serialization()),
                Ok(origin.to_owned()),
                "{entry}"
            );
        }
        let listed = ListedOrigins::parse(&entries(&["https://127.1"])).expect("parses");
        assert!(listed.admits(&HeaderValue::from_static("https://127.0.0.1")));
        assert!(
            policy(&["https://127.1"], &[])
                .allows(&from_browser("https://127.0.0.1", "api.example"))
                .is_ok()
        );
    }

    /// The URL standard parses a host as a domain or an address only over a
    /// special scheme; over any other it stays opaque, its bytes the host, an
    /// IPv6 literal aside, which it reads whatever the scheme.
    #[test]
    fn a_non_special_scheme_keeps_its_host_opaque() {
        for (entry, origin) in [
            ("foo://ABC", "foo://ABC"),
            ("FOO://ABC", "foo://ABC"),
            ("foo://127.1", "foo://127.1"),
            ("foo://0x7f.0.0.1", "foo://0x7f.0.0.1"),
            ("foo://app.1", "foo://app.1"),
            ("foo://1.2.3.256", "foo://1.2.3.256"),
            ("foo://ABC:8443", "foo://ABC:8443"),
            ("foo://[0:0:0:0:0:0:0:1]", "foo://[::1]"),
        ] {
            assert_eq!(
                Tuple::parse(entry).map(|tuple| tuple.serialization()),
                Ok(origin.to_owned()),
                "{entry}"
            );
        }
        let listed = ListedOrigins::parse(&entries(&["foo://ABC", "foo://127.1"])).expect("parses");
        let admits = |sent: &'static str| listed.admits(&HeaderValue::from_static(sent));
        assert!(admits("foo://ABC"));
        assert!(admits("foo://127.1"));
        assert!(!admits("foo://abc"), "another host, in the standard's eyes");
        assert!(!admits("foo://127.0.0.1"), "no address is read out of it");
        for (entry, refused) in [
            ("foo://", NotAnOrigin::NoHost),
            ("foo://:8443", NotAnOrigin::NoHost),
            ("foo://[nope]", NotAnOrigin::Host),
            ("foo://a%2Eb", NotAnOrigin::Host),
        ] {
            assert_eq!(Tuple::parse(entry), Err(refused), "{entry}");
        }
    }

    /// The six special schemes keep the host parser, each of them.
    #[test]
    fn a_special_scheme_reads_its_host_as_a_domain_or_an_address() {
        for scheme in ["ftp", "file", "http", "https", "ws", "wss"] {
            let of = |host: &str| Tuple::of(scheme, host).map(|tuple| tuple.host);
            assert_eq!(of("APP.example"), Ok("app.example".to_owned()), "{scheme}");
            assert_eq!(of("127.1"), Ok("127.0.0.1".to_owned()), "{scheme}");
            assert_eq!(of("app.1"), Err(NotAnOrigin::Host), "{scheme}");
        }
    }

    /// The CORS layer's reader: the bytes a browser sends, else the origin a
    /// client's spelling parses to; never `null` but under `*`.
    #[test]
    fn a_listed_origin_admits_what_parses_to_it() {
        let listed = ListedOrigins::parse(&entries(&["https://app.example"])).expect("parses");
        let admits = |sent: &'static str| listed.admits(&HeaderValue::from_static(sent));
        assert!(admits("https://app.example"));
        assert!(admits("HTTPS://APP.example:443"));
        for refused in [
            "https://app.example:8443",
            "https://app.example/",
            "https://app.example:https",
            "null",
            "app.example",
        ] {
            assert!(!admits(refused), "{refused}");
        }
        let every = ListedOrigins::parse(&entries(&["*"])).expect("parses");
        assert!(every.admits(&HeaderValue::from_static("null")));
    }

    /// A request sent to `uri`, from `peer`, carrying `headers`.
    fn request(uri: &'static str, peer: &str, headers: &[(&'static str, &'static str)]) -> Request {
        let mut builder = Request::builder()
            .uri(Uri::from_static(uri))
            .peer_addr(SocketAddr::new(
                peer.parse().expect("a test literal is an address"),
                40_000,
            ));
        for (name, value) in headers {
            builder = builder.header(
                http::HeaderName::from_static(name),
                HeaderValue::from_static(value),
            );
        }
        builder.finish()
    }

    fn from_browser(origin: &'static str, host: &'static str) -> Request {
        request(
            "/socket",
            "203.0.113.9",
            &[("origin", origin), ("host", host)],
        )
    }

    #[test]
    fn a_request_with_no_origin_is_no_browser_and_is_admitted() {
        assert!(
            policy(&[], &[])
                .allows(&request("/socket", "203.0.113.9", &[]))
                .is_ok()
        );
    }

    #[test]
    fn its_own_origin_is_admitted_with_default_ports_elided_and_hosts_without_case() {
        let policy = policy(&[], &[]);
        for (origin, host) in [
            ("http://api.example", "api.example"),
            ("http://api.example", "api.example:80"),
            ("http://api.example:80", "api.example"),
            ("HTTP://API.Example", "api.example"),
            ("http://api.example:8080", "API.EXAMPLE:8080"),
            ("http://127.0.0.1:3000", "127.0.0.1:3000"),
            ("http://[::1]:3000", "[0:0:0:0:0:0:0:1]:3000"),
        ] {
            assert!(
                policy.allows(&from_browser(origin, host)).is_ok(),
                "{origin} sent to {host}"
            );
        }
    }

    #[test]
    fn another_scheme_host_or_port_is_another_origin() {
        let policy = policy(&[], &[]);
        for (origin, reason, scheme) in [
            (
                "https://api.example",
                OriginRefusedReason::SchemeDiffers,
                OriginScheme::Https,
            ),
            (
                "ws://api.example",
                OriginRefusedReason::SchemeDiffers,
                OriginScheme::Other,
            ),
            (
                "http://api.example:8080",
                OriginRefusedReason::PortDiffers,
                OriginScheme::Http,
            ),
            (
                "http://evil.example",
                OriginRefusedReason::HostDiffers,
                OriginScheme::Http,
            ),
            (
                "http://api.example.evil.example",
                OriginRefusedReason::HostDiffers,
                OriginScheme::Http,
            ),
            (
                "http://[::2]",
                OriginRefusedReason::HostDiffers,
                OriginScheme::Http,
            ),
        ] {
            let refused = policy
                .allows(&from_browser(origin, "api.example"))
                .expect_err(origin);
            assert_eq!(refused.reason(), reason, "{origin}");
            assert_eq!(refused.scheme(), Some(scheme), "{origin}");
            assert!(!refused.trusted_proxy(), "{origin}");
        }
    }

    /// The `Origin` sent and the `Host` it was compared to are the client's:
    /// the refusal keeps neither, nor any part of them.
    #[test]
    fn a_refusal_keeps_no_value_the_request_carried() {
        let refused = policy(&[], &[PROXY])
            .allows(&request(
                "/socket",
                PROXY,
                &[
                    ("origin", "https://origin-marker.example:8443"),
                    ("host", "host-marker.example"),
                    ("x-forwarded-host", "forwarded-marker.example"),
                ],
            ))
            .expect_err("another origin");
        let kept = format!("{refused:?} {refused} {:?}", refused.hint());
        assert!(!kept.contains("marker"), "{kept}");
        assert!(!kept.contains("8443"), "{kept}");
        assert_eq!(refused.reason(), OriginRefusedReason::SchemeDiffers);
        assert!(refused.trusted_proxy());
    }

    /// The same comparison the resolution makes: behind a dual-stack listener
    /// the proxy's IPv4 entry names the `::ffff:a.b.c.d` it is reported as, so
    /// what it forwards is read.
    #[test]
    fn a_mapped_peer_is_the_proxy_its_ipv4_entry_names() {
        let forwarded = [
            ("origin", "https://api.example"),
            ("host", "api.internal:8080"),
            ("forwarded", "proto=https;host=api.example"),
        ];
        assert!(
            policy(&[], &[PROXY])
                .allows(&request("/socket", "::ffff:10.0.0.1", &forwarded))
                .is_ok()
        );
    }

    /// A proxy inside a listed range forwards as a listed address does.
    #[test]
    fn a_peer_inside_a_listed_range_is_the_proxy_it_names() {
        let forwarded = [
            ("origin", "https://api.example"),
            ("host", "api.internal:8080"),
            ("forwarded", "proto=https;host=api.example"),
        ];
        let policy = policy(&[], &["10.0.0.0/8"]);
        assert!(
            policy
                .allows(&request("/socket", "10.42.7.9", &forwarded))
                .is_ok()
        );
        assert!(
            policy
                .allows(&request("/socket", "11.0.0.1", &forwarded))
                .is_err(),
            "past the range its headers are a stranger's",
        );
    }

    /// The hint names the settings that admit the origin, with the
    /// deployment's prefix; the proxy list only while the peer is no trusted
    /// proxy, and nothing for a field no setting admits.
    #[test]
    fn the_hint_names_the_settings_that_would_admit_it() {
        let origins = nest_rs_config::var_name("http", "CORS_ORIGINS");
        let proxies = nest_rs_config::var_name("http", "TRUSTED_PROXIES");
        let hint = |trusted_proxy, reason| {
            OriginRefused::new(reason, Some(OriginScheme::Https), trusted_proxy).hint()
        };
        let direct = hint(false, OriginRefusedReason::HostDiffers).expect("a hint");
        assert!(
            direct.contains(&origins) && direct.contains(&proxies),
            "{direct}"
        );
        let proxied = hint(true, OriginRefusedReason::HostMissing).expect("a hint");
        assert!(
            proxied.contains(&origins) && !proxied.contains(&proxies),
            "{proxied}"
        );
        let null = hint(false, OriginRefusedReason::OriginNull).expect("a hint");
        assert!(null.contains(&origins) && null.contains("`*`"), "{null}");
        for reason in [
            OriginRefusedReason::OriginNotVisibleAscii,
            OriginRefusedReason::OriginNotSerialized,
            OriginRefusedReason::OriginRepeated,
        ] {
            assert_eq!(hint(false, reason), None, "{reason:?}");
        }
    }

    #[test]
    fn an_origin_that_is_not_one_is_refused() {
        let policy = policy(&["*"], &[]);
        for origin in [
            "api.example",
            "http://",
            "http://user@api.example",
            "http://api.example/path",
            "http://[::1",
            "http://api.example http://other.example",
            "http://api.example:http",
            "http://api.example:99999",
        ] {
            let refused = policy
                .allows(&from_browser(origin, "api.example"))
                .expect_err(origin);
            assert_eq!(
                refused.reason(),
                OriginRefusedReason::OriginNotSerialized,
                "{origin}"
            );
            assert_eq!(refused.scheme(), None, "{origin}");
        }
        let twice = request(
            "/socket",
            "203.0.113.9",
            &[
                ("origin", "http://api.example"),
                ("origin", "http://api.example"),
                ("host", "api.example"),
            ],
        );
        assert_eq!(
            policy.allows(&twice).map_err(|refused| refused.reason()),
            Err(OriginRefusedReason::OriginRepeated),
            "two `Origin` fields",
        );
        let mut opaque_bytes = from_browser("http://api.example", "api.example");
        opaque_bytes.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_bytes(b"http://\xe9t\xe9.example").expect("obs-text is a value"),
        );
        assert_eq!(
            policy
                .allows(&opaque_bytes)
                .map_err(|refused| refused.reason()),
            Err(OriginRefusedReason::OriginNotVisibleAscii),
        );
    }

    #[test]
    fn the_scheme_is_the_one_the_request_arrived_over() {
        let policy = policy(&[], &[]);
        let over_tls = request(
            "https://api.example/socket",
            "203.0.113.9",
            &[("origin", "https://api.example"), ("host", "api.example")],
        );
        assert!(policy.allows(&over_tls).is_ok());
        let downgraded = request(
            "https://api.example/socket",
            "203.0.113.9",
            &[("origin", "http://api.example"), ("host", "api.example")],
        );
        assert!(policy.allows(&downgraded).is_err());
    }

    #[test]
    fn with_no_host_the_authority_the_target_named_is_read() {
        let policy = policy(&[], &[]);
        let h2 = request(
            "https://api.example/socket",
            "203.0.113.9",
            &[("origin", "https://api.example")],
        );
        assert!(policy.allows(&h2).is_ok());
        let nowhere = request(
            "/socket",
            "203.0.113.9",
            &[("origin", "https://api.example")],
        );
        let refused = policy
            .allows(&nowhere)
            .expect_err("no host to compare with");
        assert_eq!(refused.reason(), OriginRefusedReason::HostMissing);
        assert_eq!(refused.scheme(), Some(OriginScheme::Https));
    }

    /// No list may name `null`, so an opaque origin is admitted by `*` alone.
    #[test]
    fn an_opaque_origin_is_admitted_by_the_wildcard_alone() {
        let null = from_browser("null", "api.example");
        assert_eq!(
            policy(&[], &[])
                .allows(&null)
                .map_err(|refused| refused.reason()),
            Err(OriginRefusedReason::OriginNull),
        );
        assert!(policy(&["https://app.example"], &[]).allows(&null).is_err());
        assert!(
            policy(&["*"], &[]).allows(&null).is_ok(),
            "`*` lists every origin"
        );
    }

    /// An internationalized host is listed as browsers send it, in IDNA's
    /// ASCII form.
    #[test]
    fn an_internationalized_host_listed_in_its_ascii_form_admits_what_browsers_send() {
        let listed = policy(&["https://xn--9ca.example"], &[]);
        assert!(
            listed
                .allows(&from_browser("https://xn--9ca.example", "api.example"))
                .is_ok()
        );
    }

    #[test]
    fn a_listed_origin_is_admitted_and_a_wildcard_admits_every_origin() {
        let app = from_browser("https://app.example", "api.example");
        let evil = from_browser("https://evil.example", "api.example");
        let listed = policy(&["https://app.example:443"], &[]);
        assert!(listed.allows(&app).is_ok());
        assert!(listed.allows(&evil).is_err());
        let every = policy(&["https://app.example", "*"], &[]);
        assert!(every.allows(&evil).is_ok());
    }

    #[test]
    fn a_forwarded_host_and_scheme_are_ignored_from_an_untrusted_peer() {
        let policy = policy(&[], &[PROXY]);
        let forged = request(
            "/socket",
            "203.0.113.9",
            &[
                ("origin", "https://evil.example"),
                ("host", "api.internal:8080"),
                ("forwarded", "proto=https;host=evil.example"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-host", "evil.example"),
            ],
        );
        let refused = policy.allows(&forged).expect_err("headers from a stranger");
        assert_eq!(
            refused.reason(),
            OriginRefusedReason::SchemeDiffers,
            "compared with `http://api.internal:8080`, what the stranger reached",
        );
        assert!(!refused.trusted_proxy());
    }

    #[test]
    fn a_forwarded_host_and_scheme_are_honoured_from_a_trusted_proxy() {
        let policy = policy(&[], &[PROXY]);
        for headers in [
            &[
                ("origin", "https://api.example"),
                ("host", "api.internal:8080"),
                (
                    "forwarded",
                    "for=203.0.113.9;proto=https;host=\"api.example\"",
                ),
            ][..],
            &[
                ("origin", "https://api.example"),
                ("host", "api.internal:8080"),
                ("x-forwarded-proto", "https, http"),
                ("x-forwarded-host", "api.example, api.internal"),
            ][..],
        ] {
            assert!(
                policy.allows(&request("/socket", PROXY, headers)).is_ok(),
                "{headers:?}"
            );
        }
        let rewritten = request(
            "/socket",
            PROXY,
            &[
                ("origin", "https://api.example"),
                ("host", "api.internal:8080"),
            ],
        );
        let refused = policy
            .allows(&rewritten)
            .expect_err("a proxy that rewrote `Host` and said nothing");
        assert_eq!(
            refused.reason(),
            OriginRefusedReason::SchemeDiffers,
            "compared with `http://api.internal:8080`, the `Host` it rewrote",
        );
        assert!(refused.trusted_proxy());
    }

    /// The handshake admits `websocket` anywhere in `Upgrade`'s list, in any
    /// case and on any line: each such request is checked before it.
    #[tokio::test]
    async fn every_upgrade_the_handshake_admits_is_checked() {
        let checked = OriginChecked::new(
            policy(&[], &[]),
            OriginGate {
                on: OriginCheck::Upgrades,
                refused: |_| {},
            },
            BoxEndpoint::new(crate::endpoint_fn(|_req| async {
                Ok(crate::Response::default())
            })),
        );
        let foreign = [("origin", "https://evil.example"), ("host", "api.example")];
        for lines in [
            &["websocket"][..],
            &["WebSocket"],
            &["websocket, h2c"],
            &["h2c ,websocket"],
            &["h2c", "websocket"],
        ] {
            let mut headers = foreign.to_vec();
            headers.extend(lines.iter().map(|&line| ("upgrade", line)));
            let resp = checked
                .call(request("/socket", "203.0.113.9", &headers))
                .await
                .expect("an answer");
            assert_eq!(resp.status(), http::StatusCode::FORBIDDEN, "{lines:?}");
        }
        for lines in [&[][..], &["h2c"], &["websockets"]] {
            let mut headers = foreign.to_vec();
            headers.extend(lines.iter().map(|&line| ("upgrade", line)));
            let resp = checked
                .call(request("/socket", "203.0.113.9", &headers))
                .await
                .expect("an answer");
            assert_eq!(resp.status(), http::StatusCode::OK, "{lines:?}");
        }
    }

    #[tokio::test]
    async fn a_refusal_answers_a_forbidden_problem_document_naming_no_origin() {
        let refused = policy(&[], &[])
            .allows(&from_browser("https://evil.example", "api.example"))
            .expect_err("another origin");
        assert!(!refused.to_string().contains("evil"), "{refused}");
        let (head, body) = refused.as_response().into_parts();
        assert_eq!(head.status, http::StatusCode::FORBIDDEN);
        assert_eq!(
            head.headers[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body = body.into_string().await.expect("a JSON body");
        let document: serde_json::Value = serde_json::from_str(&body).expect("a problem document");
        assert_eq!(document["code"], "FORBIDDEN");
        assert_eq!(document["detail"], ORIGIN_REFUSED);
        assert!(!body.contains("evil"), "{body}");
    }
}
