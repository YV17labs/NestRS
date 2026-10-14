//! [`OriginPolicy`] — whether a browser's `Origin` may open an upgrade or an
//! MCP exchange, the requests a CORS policy cannot protect: a browser consults
//! CORS before it lets a page read a response, never before it opens a socket
//! or sends a request whose effect is the point.
//!
//! The policy is the transport's own configuration — its CORS origins and its
//! trusted proxies — resolved once per mount, so with CORS configured it
//! repeats the answer the transport's CORS layer already gave, and an entry
//! point mounted outside the transport is held to it all the same.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

use http::header::{self, HeaderMap};
use http::uri::{Authority, Scheme};
use nest_rs_core::problem::code;
use nest_rs_core::{Container, Problem};

use crate::HttpConfig;
use crate::client_ip::forwarded_param;
use crate::cors::WILDCARD;
use crate::error::ResponseError;
use crate::response::Response;

/// The de-facto forwarding headers for the scheme and the host, which the
/// `http` crate does not name.
const X_FORWARDED_PROTO: &str = "x-forwarded-proto";
const X_FORWARDED_HOST: &str = "x-forwarded-host";

/// What a refusal says, to the client and in its error: never the origin sent.
const REFUSED: &str =
    "the request's origin is neither this server's own nor one its CORS policy lists";

/// Whether a browser `Origin` may open an upgrade or an MCP exchange: absent ⇒
/// yes, no browser sent it; present ⇒ it is the request's own origin, or the
/// CORS policy lists it (`*` lists every origin — an explicit opening).
///
/// "The request's own origin" is `scheme://host[:port]`: the scheme the
/// request arrived over and its `Host` — or, from a trusted proxy alone, what
/// `Forwarded` (`proto=`, `host=`) or `X-Forwarded-Proto` / `X-Forwarded-Host`
/// say the client asked for. Origins compare per RFC 6454 §5: scheme and host
/// without case, ports with the scheme's default filled in.
///
/// ```
/// use nest_rs_core::Container;
/// use nest_rs_http::__private::OriginPolicy;
/// use nest_rs_http::{HeaderValue, Request, header};
///
/// let policy = OriginPolicy::from_container(&Container::builder().build());
/// let browser = |origin: &'static str| {
///     Request::builder()
///         .header(header::HOST, HeaderValue::from_static("api.example"))
///         .header(header::ORIGIN, HeaderValue::from_static(origin))
///         .finish()
/// };
///
/// assert!(policy.allows(&Request::builder().finish()).is_ok(), "no browser");
/// assert!(policy.allows(&browser("http://api.example")).is_ok());
/// assert!(policy.allows(&browser("https://evil.example")).is_err());
/// ```
#[derive(Clone, Debug, Default)]
pub struct OriginPolicy {
    listed: Listed,
    trusted_proxies: Box<[IpAddr]>,
}

/// The origins a CORS policy lists, beside the request's own.
#[derive(Clone, Debug)]
enum Listed {
    /// Each origin, parsed once. An entry that is no origin serialization
    /// lists nothing, as it lists nothing to the CORS layer.
    Origins(Box<[Origin]>),
    /// `*`.
    Every,
}

impl Default for Listed {
    fn default() -> Self {
        Self::Origins(Box::default())
    }
}

impl OriginPolicy {
    /// The policy the app's transport serves under, read off the
    /// [`HttpConfig`] the app booted with; an app importing no `HttpModule`
    /// admits the request's own origin alone, behind no trusted proxy.
    pub fn from_container(container: &Container) -> Self {
        container
            .get::<HttpConfig>()
            .map_or_else(Self::default, |config| Self::from_config(&config))
    }

    /// The policy `config` describes.
    pub(crate) fn from_config(config: &HttpConfig) -> Self {
        let listed = match &config.cors {
            None => Listed::default(),
            Some(cors) if cors.origins.iter().any(|origin| origin == WILDCARD) => Listed::Every,
            Some(cors) => Listed::Origins(
                cors.origins
                    .iter()
                    .filter_map(|origin| Origin::parse(origin))
                    .collect(),
            ),
        };
        Self {
            listed,
            trusted_proxies: config.trusted_proxies.clone().into_boxed_slice(),
        }
    }

    /// `Ok` when `request` carries no `Origin`, its own, or one the CORS policy
    /// lists; otherwise the refusal, which answers `403`. A second `Origin`
    /// field, or one that is no origin serialization, is refused.
    pub fn allows(&self, request: &impl OriginFacts) -> Result<(), OriginRefused> {
        let mut sent = request.headers().get_all(header::ORIGIN).iter();
        let Some(origin) = sent.next() else {
            return Ok(());
        };
        let parsed = match sent.next() {
            None => origin.to_str().ok().and_then(Origin::parse),
            Some(_) => None,
        };
        let own = self.own(request);
        if let Some(parsed) = &parsed
            && (own.as_ref() == Some(parsed) || self.lists(parsed))
        {
            return Ok(());
        }
        Err(OriginRefused {
            origin: origin
                .to_str()
                .map_or_else(|_| "(not visible ASCII)".to_owned(), ToOwned::to_owned),
            expected: own.map_or_else(|| "none".to_owned(), |own| own.to_string()),
        })
    }

    fn lists(&self, origin: &Origin) -> bool {
        match &self.listed {
            Listed::Every => true,
            Listed::Origins(origins) => origins.contains(origin),
        }
    }

    /// The origin the request was sent to.
    fn own(&self, request: &impl OriginFacts) -> Option<Origin> {
        let headers = request.headers();
        let proxied = request
            .peer()
            .is_some_and(|peer| self.trusted_proxies.contains(&peer));
        let (proto, host) = if proxied {
            forwarded(headers)
        } else {
            (None, None)
        };
        let scheme = proto.unwrap_or_else(|| request.scheme().as_str());
        let host = host
            .or_else(|| headers.get(header::HOST)?.to_str().ok())
            .or_else(|| request.authority())?;
        Origin::tuple(scheme, host)
    }
}

/// What a trusted proxy says the client asked for: `Forwarded`'s `proto` and
/// `host` (RFC 7239 §5.3, §5.4), else `X-Forwarded-Proto` and
/// `X-Forwarded-Host` — each the first value, which the hop facing the client
/// wrote. A value a caller prepended can only claim an origin, and a caller
/// that is no browser may send none at all.
fn forwarded(headers: &HeaderMap) -> (Option<&str>, Option<&str>) {
    let standard = headers
        .get(header::FORWARDED)
        .and_then(|value| value.to_str().ok());
    let de_facto = |name: &str| {
        headers
            .get(name)?
            .to_str()
            .ok()?
            .split(',')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let named = |param: &str, header: &str| {
        standard
            .and_then(|raw| forwarded_param(raw, param))
            .or_else(|| de_facto(header))
    };
    (
        named("proto", X_FORWARDED_PROTO),
        named("host", X_FORWARDED_HOST),
    )
}

/// What [`OriginPolicy::allows`] reads off a request: its headers, the
/// authority its target named, the scheme it arrived over and its peer.
pub trait OriginFacts {
    /// The request's header fields.
    fn headers(&self) -> &HeaderMap;
    /// The authority the request target named — HTTP/2's `:authority` — when
    /// it named one.
    fn authority(&self) -> Option<&str>;
    /// The scheme the request arrived over.
    fn scheme(&self) -> &Scheme;
    /// The address of the direct peer, when there is one.
    fn peer(&self) -> Option<IpAddr>;
}

impl OriginFacts for crate::Request {
    fn headers(&self) -> &HeaderMap {
        crate::Request::headers(self)
    }

    fn authority(&self) -> Option<&str> {
        self.original_uri().authority().map(Authority::as_str)
    }

    fn scheme(&self) -> &Scheme {
        crate::Request::scheme(self)
    }

    fn peer(&self) -> Option<IpAddr> {
        self.peer_addr().map(|addr| addr.ip())
    }
}

impl OriginFacts for poem::Request {
    fn headers(&self) -> &HeaderMap {
        poem::Request::headers(self)
    }

    fn authority(&self) -> Option<&str> {
        self.original_uri().authority().map(Authority::as_str)
    }

    fn scheme(&self) -> &Scheme {
        poem::Request::scheme(self)
    }

    fn peer(&self) -> Option<IpAddr> {
        self.remote_addr().as_socket_addr().map(|addr| addr.ip())
    }
}

/// A browser `Origin` the policy refused: it answers `403` with a problem
/// document that names no origin, and carries what the edge's `warn` says.
#[derive(Debug, thiserror::Error)]
#[error("{}", REFUSED)]
pub struct OriginRefused {
    origin: String,
    expected: String,
}

impl OriginRefused {
    /// The `Origin` the browser sent, as the header spelled it.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The origin the server took for its own, or `none` when the request
    /// named no host — what tells an operator a proxy is not trusted yet.
    pub fn expected(&self) -> &str {
        &self.expected
    }
}

impl ResponseError for OriginRefused {
    fn status(&self) -> http::StatusCode {
        http::StatusCode::FORBIDDEN
    }

    fn as_response(&self) -> Response {
        crate::problem::problem_response(&Problem::new(403, code::FORBIDDEN).with_detail(REFUSED))
    }
}

/// An origin (RFC 6454 §4), in the form two compare in (§5).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Origin {
    /// `null`, an opaque origin (§7.3): admitted only where `null` is listed.
    Opaque,
    /// Scheme and host lowercased, IPv6 literals in their canonical text, and
    /// the port the scheme defaults to when none is written.
    Tuple {
        scheme: String,
        host: String,
        port: Option<u16>,
    },
}

impl Origin {
    /// An `Origin` serialization (§6.2): `null`, or `scheme "://" host [ ":"
    /// port ]` with no path.
    fn parse(serialized: &str) -> Option<Self> {
        if serialized == "null" {
            return Some(Self::Opaque);
        }
        let (scheme, authority) = serialized.split_once("://")?;
        Self::tuple(scheme, authority)
    }

    /// The origin of a request sent to `authority` over `scheme`.
    fn tuple(scheme: &str, authority: &str) -> Option<Self> {
        let scheme_is_valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
        // Userinfo is no part of an origin, nor of a `Host`.
        if !scheme_is_valid || authority.contains('@') {
            return None;
        }
        let authority: Authority = authority.parse().ok()?;
        let host = canonical_host(authority.host())?;
        let scheme = scheme.to_ascii_lowercase();
        let port = authority.port_u16().or_else(|| default_port(&scheme));
        Some(Self::Tuple { scheme, host, port })
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opaque => f.write_str("null"),
            Self::Tuple { scheme, host, port } => {
                write!(f, "{scheme}://{host}")?;
                match port {
                    Some(port) if Some(*port) != default_port(scheme) => write!(f, ":{port}"),
                    _ => Ok(()),
                }
            }
        }
    }
}

/// `host` as two origins compare it: lowercase, an IPv6 literal in its
/// canonical text; `None` when it is empty or a malformed literal.
fn canonical_host(host: &str) -> Option<String> {
    if let Some(literal) = host.strip_prefix('[') {
        let address: Ipv6Addr = literal.strip_suffix(']')?.parse().ok()?;
        return Some(format!("[{address}]"));
    }
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
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
    use crate::{HttpCors, Request};

    const PROXY: &str = "10.0.0.1";

    fn policy(origins: &[&str], trusted: &[&str]) -> OriginPolicy {
        OriginPolicy::from_config(&HttpConfig {
            cors: (!origins.is_empty()).then(|| HttpCors {
                origins: origins.iter().map(|&origin| origin.to_owned()).collect(),
                ..HttpCors::default()
            }),
            trusted_proxies: trusted
                .iter()
                .map(|ip| ip.parse().expect("a test literal is an address"))
                .collect(),
            ..HttpConfig::default()
        })
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
        for origin in [
            "https://api.example",
            "http://api.example:8080",
            "http://evil.example",
            "http://api.example.evil.example",
            "http://[::2]",
        ] {
            let refused = policy
                .allows(&from_browser(origin, "api.example"))
                .expect_err(origin);
            assert_eq!(refused.origin(), origin);
            assert_eq!(refused.expected(), "http://api.example");
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
        ] {
            assert!(
                policy.allows(&from_browser(origin, "api.example")).is_err(),
                "{origin}"
            );
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
        assert!(policy.allows(&twice).is_err(), "two `Origin` fields");
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
        assert_eq!(refused.expected(), "none");
    }

    #[test]
    fn an_opaque_origin_is_admitted_only_where_null_is_listed() {
        let null = from_browser("null", "api.example");
        assert!(policy(&[], &[]).allows(&null).is_err());
        assert!(policy(&["https://app.example"], &[]).allows(&null).is_err());
        assert!(policy(&["null"], &[]).allows(&null).is_ok());
        assert!(
            policy(&["*"], &[]).allows(&null).is_ok(),
            "`*` lists every origin"
        );
    }

    #[test]
    fn a_listed_origin_is_admitted_and_a_wildcard_admits_every_origin() {
        let app = from_browser("https://app.example", "api.example");
        let evil = from_browser("https://evil.example", "api.example");
        let listed = policy(&["https://app.example:443", "not an origin"], &[]);
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
        assert_eq!(refused.expected(), "http://api.internal:8080");
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
        assert_eq!(refused.expected(), "http://api.internal:8080");
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
        assert_eq!(document["detail"], REFUSED);
        assert!(!body.contains("evil"), "{body}");
    }
}
