//! CORS settings for the HTTP transport, read from `<PREFIX>_HTTP__CORS_*` or
//! pinned as `HttpConfig.cors`, and checked at boot by the transport, which
//! answers the policy around every route.

use std::time::Duration;

use anyhow::Result;
use http::Method;
use nest_rs_config::ConfigService;

/// Cross-Origin Resource Sharing policy. `origins` empty ⇒ no CORS layer
/// installed (the default). Lists are comma-separated in env vars.
///
/// The same `origins` decide which browser origins open a WebSocket gateway's
/// socket or a graphql-ws socket and call `/mcp`, which a browser never asks
/// CORS about; the CORS layer answers first and admits only what they list,
/// the server's own origin included.
#[derive(Clone, Debug, Default)]
pub struct HttpCors {
    /// Allowed origins; empty ⇒ no CORS layer installed. Also the browser
    /// origins the socket upgrades and `/mcp` admit.
    pub origins: Vec<String>,
    /// Allowed request methods (`Access-Control-Allow-Methods`); empty ⇒ the
    /// methods the routes serve and the CORS-safelisted `GET`, `HEAD`,
    /// `POST`, or every method a page may send (`PUT`, `DELETE`, `PATCH` and
    /// `OPTIONS` beside those) once an address answers any method itself, as
    /// `/mcp` does. A method a page cannot send (`CONNECT`, `TRACE`, `TRACK`)
    /// fails the boot.
    pub methods: Vec<String>,
    /// Allowed request headers (`Access-Control-Allow-Headers`).
    pub headers: Vec<String>,
    /// Response headers exposed to the browser (`Access-Control-Expose-Headers`),
    /// beside the ones the framework itself sends, which are always exposed.
    pub exposed_headers: Vec<String>,
    /// Whether to allow credentialed requests (`Access-Control-Allow-Credentials`).
    pub credentials: bool,
    /// Preflight cache lifetime (`Access-Control-Max-Age`); `None` omits it.
    pub max_age: Option<Duration>,
}

/// The wildcard, as the four lists spell it.
pub(crate) const WILDCARD: &str = "*";

/// The response headers the framework sends on a route's behalf. None is
/// CORS-safelisted, so a cross-origin client reads them only when exposed.
pub(crate) const FRAMEWORK_SENT: [&str; 3] = ["link", "location", "retry-after"];

/// The methods WHATWG Fetch forbids a page to send (*forbidden method*), which
/// no preflight asks for.
const FORBIDDEN_METHODS: [&str; 3] = ["CONNECT", "TRACE", "TRACK"];

/// The CORS-safelisted methods (WHATWG Fetch): a request needs no preflight for
/// one, but a preflight it needs for a header still checks it.
const SAFELISTED_METHODS: [Method; 3] = [Method::GET, Method::HEAD, Method::POST];

/// The methods a page may send an address that dispatches on the method
/// itself: RFC 9110's and `PATCH` (RFC 5789), the forbidden ones aside.
const SENDABLE_METHODS: [Method; 7] = [
    Method::GET,
    Method::HEAD,
    Method::POST,
    Method::PUT,
    Method::DELETE,
    Method::OPTIONS,
    Method::PATCH,
];

/// Whether `method` is one a page cannot send, matched without case.
fn is_forbidden(method: &str) -> bool {
    FORBIDDEN_METHODS
        .iter()
        .any(|forbidden| forbidden.eq_ignore_ascii_case(method))
}

/// What a policy naming no method admits: the CORS-safelisted methods, each
/// method `routed` names, and every method a page may send when an address
/// answers `every_method` itself — never a forbidden one.
pub(crate) fn admitted_by_default<'a>(
    routed: impl IntoIterator<Item = &'a Method>,
    every_method: bool,
) -> Vec<Method> {
    let mut admitted = match every_method {
        true => SENDABLE_METHODS.to_vec(),
        false => SAFELISTED_METHODS.to_vec(),
    };
    for method in routed {
        if !is_forbidden(method.as_str()) && !admitted.contains(method) {
            admitted.push(method.clone());
        }
    }
    admitted
}

/// Which check of the policy a cross-origin request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CorsRefusal {
    /// Its `Origin` is not listed.
    Origin,
    /// The method its preflight asks for is not admitted.
    Method,
    /// A header its preflight asks for is not admitted.
    Headers,
}

impl CorsRefusal {
    /// The `reason` an operator filters the refusal on.
    fn as_str(self) -> &'static str {
        match self {
            Self::Origin => "origin_not_allowed",
            Self::Method => "method_not_allowed",
            Self::Headers => "headers_not_allowed",
        }
    }

    /// The one `warn` a refusal files: the check it failed, and none of what
    /// the request sent, whose origin is the client's to choose.
    pub(crate) fn file(self) {
        tracing::warn!(
            target: crate::target::HTTP,
            reason = self.as_str(),
            "cross-origin request refused by the CORS policy",
        );
    }
}

/// Whether a list holds the wildcard, which admits every value.
pub(crate) fn holds_wildcard(values: &[String]) -> bool {
    values.iter().any(|value| value == WILDCARD)
}

/// The values a list admits one by one: none when it holds the wildcard, which
/// admits them all.
pub(crate) fn listed(values: &[String]) -> &[String] {
    if holds_wildcard(values) { &[] } else { values }
}

/// A list a `*` may appear in, paired with the response header it renders into.
type WildcardList<'a> = (&'a str, &'a str, &'a [String]);

impl HttpCors {
    /// Every list where `*` is a literal once the request's credentials mode is
    /// `include` — WHATWG Fetch, *CORS protocol*. `*` is a valid `tchar`, so
    /// nothing else refuses it: the header is emitted and every browser drops it.
    fn wildcard_lists(&self) -> [WildcardList<'_>; 4] {
        [
            ("origins", "Access-Control-Allow-Origin", &self.origins),
            ("headers", "Access-Control-Allow-Headers", &self.headers),
            ("methods", "Access-Control-Allow-Methods", &self.methods),
            (
                "exposed_headers",
                "Access-Control-Expose-Headers",
                &self.exposed_headers,
            ),
        ]
    }

    /// Refuse a method no page can send: a policy listing it would claim to
    /// admit what no browser asks for.
    pub(crate) fn reject_forbidden_methods(&self) -> Result<()> {
        if let Some(method) = self.methods.iter().find(|method| is_forbidden(method)) {
            anyhow::bail!(
                "invalid CORS config: `{}` in `methods` is a forbidden method — WHATWG Fetch \
                 never lets a page send it, so no preflight asks for it; drop it from the list",
                method.to_ascii_uppercase(),
            );
        }
        Ok(())
    }

    /// Refuse a `*` that credentials turn into a literal no browser honours.
    pub(crate) fn reject_wildcards_with_credentials(&self) -> Result<()> {
        if !self.credentials {
            return Ok(());
        }
        for (field, header, values) in self.wildcard_lists() {
            if holds_wildcard(values) {
                anyhow::bail!(
                    "invalid CORS config: `{WILDCARD}` in `{field}` with `credentials` — for a \
                     request whose credentials mode is `include`, WHATWG Fetch reads \
                     `{WILDCARD}` in `{header}` as that literal value rather than as a \
                     wildcard, so the response is emitted and no browser honours it; list the \
                     values explicitly or turn `credentials` off",
                );
            }
        }
        Ok(())
    }

    /// Overlay the `<PREFIX>_HTTP__CORS_*` keys onto `base` (the policy pinned in
    /// code, if any). Returns `Ok(None)` when neither the environment nor `base`
    /// supplies an origin — no origins means no CORS layer. Each sub-key
    /// overlays independently.
    pub fn from_env(
        env: &ConfigService,
        base: Option<Self>,
    ) -> nest_rs_config::Result<Option<Self>> {
        let base = base.unwrap_or_default();
        let origins = env.list("CORS_ORIGINS", base.origins)?;
        if origins.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            origins,
            methods: env.list("CORS_METHODS", base.methods)?,
            headers: env.list("CORS_HEADERS", base.headers)?,
            exposed_headers: env.list("CORS_EXPOSED", base.exposed_headers)?,
            credentials: env.flag("CORS_CREDENTIALS", base.credentials)?,
            max_age: env
                .parse::<u64>("CORS_MAX_AGE")?
                .map(Duration::from_secs)
                .or(base.max_age),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_admits_the_safelisted_and_routed_methods_never_a_forbidden_one() {
        let track = Method::from_bytes(b"TRACK").unwrap();
        let routed = [Method::PUT, Method::GET, Method::TRACE, Method::PUT, track];
        assert_eq!(
            admitted_by_default(&routed, false),
            [Method::GET, Method::HEAD, Method::POST, Method::PUT],
        );
    }

    #[test]
    fn an_address_answering_every_method_admits_every_method_a_page_sends() {
        let propfind = Method::from_bytes(b"PROPFIND").unwrap();
        let admitted = admitted_by_default([&propfind], true);
        for method in [Method::PUT, Method::DELETE, Method::PATCH, propfind] {
            assert!(admitted.contains(&method), "{method}");
        }
        assert!(!admitted.contains(&Method::TRACE));
        assert!(!admitted.contains(&Method::CONNECT));
    }

    #[test]
    fn from_env_returns_none_when_origins_unset() {
        let cfg =
            HttpCors::from_env(&ConfigService::with_vars("http", []), None).expect("no error");
        assert!(cfg.is_none(), "unset origins ⇒ CORS off");
    }

    #[test]
    fn from_env_reads_origins_methods_headers_when_set() {
        let service = ConfigService::with_vars(
            "http",
            [
                ("CORS_ORIGINS", "https://a.example,https://b.example"),
                ("CORS_METHODS", "GET,POST"),
                ("CORS_HEADERS", "content-type"),
            ],
        );
        let cfg = HttpCors::from_env(&service, None)
            .expect("no error")
            .expect("Some when origins set");
        assert_eq!(
            cfg.origins,
            vec!["https://a.example".to_string(), "https://b.example".into()]
        );
        assert_eq!(cfg.methods, vec!["GET".to_string(), "POST".into()]);
        assert_eq!(cfg.headers, vec!["content-type".to_string()]);
        assert!(!cfg.credentials, "off by default");
        assert!(cfg.max_age.is_none(), "off by default");
    }

    #[test]
    fn from_env_reads_credentials_flag_and_max_age() {
        let service = ConfigService::with_vars(
            "http",
            [
                ("CORS_ORIGINS", "*"),
                ("CORS_CREDENTIALS", "true"),
                ("CORS_MAX_AGE", "600"),
            ],
        );
        let cfg = HttpCors::from_env(&service, None)
            .expect("no error")
            .expect("Some");
        assert!(cfg.credentials);
        assert_eq!(cfg.max_age, Some(Duration::from_secs(600)));
    }
}
