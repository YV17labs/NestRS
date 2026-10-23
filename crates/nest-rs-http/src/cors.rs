//! CORS settings for the HTTP transport, read from `<PREFIX>_HTTP__CORS_*` or
//! pinned as `HttpConfig.cors`, checked at boot into a [`CorsPolicy`], and
//! answered around every route by [`Cors`] (WHATWG Fetch, *CORS protocol*).

use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN,
};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use nest_rs_config::ConfigService;

use crate::endpoint::Endpoint;
use crate::error::HttpError;
use crate::problem::ProblemDetails;
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::vary::vary_on;

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
#[derive(Clone, Copy, Debug)]
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

    /// What the refusal's problem document says: the check, never a value.
    fn sentence(self) -> &'static str {
        match self {
            Self::Origin => "request-origin not allowed",
            Self::Method => "request-method not allowed",
            Self::Headers => "request-headers not allowed",
        }
    }

    /// The one `warn` a refusal files: the check it failed, and none of what
    /// the request sent, whose origin is the client's to choose.
    fn file(self) {
        tracing::warn!(
            target: crate::target::HTTP,
            reason = self.as_str(),
            "cross-origin request refused by the CORS policy",
        );
    }

    /// The refusal filed, and answered: `403` and its sentence.
    fn answer(self) -> Response {
        self.file();
        ProblemDetails::from_status(StatusCode::FORBIDDEN)
            .with_detail(self.sentence())
            .into_response()
    }
}

/// A CORS policy checked at boot: it answers nothing until it knows the route
/// table it serves ([`CorsPolicy::serving`]).
pub(crate) struct CorsPolicy {
    /// The origins admitted one by one; none admits every origin.
    origins: Vec<HeaderValue>,
    methods: Admitted,
    /// The request headers admitted; none admits every header a preflight
    /// asks for.
    headers: Vec<HeaderName>,
    /// `Access-Control-Expose-Headers`: what the framework sends, beside what
    /// the policy lists.
    exposed: HeaderValue,
    credentials: bool,
    max_age: Option<HeaderValue>,
}

/// The methods a CORS policy admits.
enum Admitted {
    /// The ones it lists.
    Listed(Vec<Method>),
    /// Every one, said as `*`.
    Every,
    /// The ones the route table serves, read once it is built.
    Routed,
}

impl CorsPolicy {
    /// The policy as it serves the route table: one listing no method admits
    /// the CORS-safelisted ones and those `routed` names, or every method a
    /// page sends when an address answers `every_method` itself.
    pub(crate) fn serving<'a>(
        self,
        routed: impl IntoIterator<Item = &'a Method>,
        every_method: bool,
    ) -> ServedCorsPolicy {
        let methods = match self.methods {
            Admitted::Listed(methods) => Some(methods),
            Admitted::Every => None,
            Admitted::Routed => Some(admitted_by_default(routed, every_method)),
        };
        let allow_methods = match &methods {
            Some(methods) => joined(methods.iter().map(Method::as_str)),
            None => HeaderValue::from_static(WILDCARD),
        };
        let allow_headers =
            (!self.headers.is_empty()).then(|| joined(self.headers.iter().map(HeaderName::as_str)));
        ServedCorsPolicy {
            origins: self.origins,
            methods,
            allow_methods,
            headers: self.headers,
            allow_headers,
            exposed: self.exposed,
            credentials: self.credentials,
            max_age: self.max_age,
        }
    }
}

/// A CORS policy serving one route table, every header its answers carry
/// built once.
pub(crate) struct ServedCorsPolicy {
    origins: Vec<HeaderValue>,
    /// The methods a preflight may ask for; `None` admits every one.
    methods: Option<Vec<Method>>,
    allow_methods: HeaderValue,
    headers: Vec<HeaderName>,
    /// `Access-Control-Allow-Headers` when the policy lists headers; `None`
    /// echoes what a preflight asks for.
    allow_headers: Option<HeaderValue>,
    exposed: HeaderValue,
    credentials: bool,
    max_age: Option<HeaderValue>,
}

impl ServedCorsPolicy {
    /// `inner`, answered under this policy.
    pub(crate) fn around<E>(self, inner: E) -> Cors<E> {
        Cors {
            inner,
            policy: self,
        }
    }

    /// Whether a preflight asking for `requested` may send them: one of
    /// them listed is enough, the answer naming what is.
    fn admits_headers(&self, requested: Option<&HeaderValue>) -> bool {
        let Some(requested) = requested.filter(|_| !self.headers.is_empty()) else {
            return true;
        };
        requested.to_str().is_ok_and(|names| {
            names
                .split(',')
                .filter_map(|name| HeaderName::from_str(name.trim()).ok())
                .any(|name| self.headers.contains(&name))
        })
    }

    /// The answer to an admitted preflight from `origin`.
    fn preflight(&self, origin: HeaderValue, requested: Option<&HeaderValue>) -> Response {
        let mut resp = Response::builder()
            .header(ACCESS_CONTROL_ALLOW_ORIGIN, origin)
            .header(ACCESS_CONTROL_EXPOSE_HEADERS, self.exposed.clone())
            .header(ACCESS_CONTROL_ALLOW_METHODS, self.allow_methods.clone())
            .header(
                ACCESS_CONTROL_ALLOW_HEADERS,
                match (&self.allow_headers, requested) {
                    (Some(listed), _) => listed.clone(),
                    (None, Some(requested)) => requested.clone(),
                    (None, None) => HeaderValue::from_static(WILDCARD),
                },
            );
        if let Some(max_age) = &self.max_age {
            resp = resp.header(ACCESS_CONTROL_MAX_AGE, max_age.clone());
        }
        if self.credentials {
            resp = resp.header(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
        resp.finish()
    }
}

/// An endpoint answered under a CORS policy. Every answer depends on the
/// request's `Origin`, whatever the policy lists, a refusal included, so each
/// varies on it: a shared cache keys it on the origin (WHATWG Fetch, *CORS
/// protocol and HTTP caches*).
pub(crate) struct Cors<E> {
    inner: E,
    policy: ServedCorsPolicy,
}

impl<E: Endpoint> Cors<E> {
    /// The answer to a request carrying `origin`, before it varies.
    async fn answer(&self, req: Request, origin: HeaderValue) -> Response {
        let policy = &self.policy;
        if !policy.origins.is_empty() && !policy.origins.contains(&origin) {
            return CorsRefusal::Origin.answer();
        }
        if req.method() == Method::OPTIONS {
            let admitted = req
                .headers()
                .get(ACCESS_CONTROL_REQUEST_METHOD)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<Method>().ok())
                .is_some_and(|method| {
                    policy
                        .methods
                        .as_ref()
                        .is_none_or(|methods| methods.contains(&method))
                });
            if !admitted {
                return CorsRefusal::Method.answer();
            }
            let requested = req.headers().get(ACCESS_CONTROL_REQUEST_HEADERS);
            if !policy.admits_headers(requested) {
                return CorsRefusal::Headers.answer();
            }
            return policy.preflight(origin, requested);
        }
        let mut resp = self.inner.call(req).await.unwrap_or_else(HttpError::answer);
        let headers = resp.headers_mut();
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        if policy.credentials {
            headers.insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
        headers.insert(ACCESS_CONTROL_EXPOSE_HEADERS, policy.exposed.clone());
        resp
    }
}

impl<E: Endpoint> Endpoint for Cors<E> {
    async fn call(&self, req: Request) -> crate::Result<Response> {
        let mut resp = match req.headers().get(ORIGIN).cloned() {
            Some(origin) => self.answer(req, origin).await,
            // No `Origin`, no CORS request.
            None => self.inner.call(req).await.unwrap_or_else(HttpError::answer),
        };
        vary_on(resp.headers_mut(), ORIGIN);
        Ok(resp)
    }
}

/// `values` as one field value, comma-separated; each is a method or a header
/// name, both tokens, so the value is always one a header carries.
fn joined<'a>(values: impl Iterator<Item = &'a str>) -> HeaderValue {
    let joined = values.collect::<Vec<_>>().join(", ");
    HeaderValue::try_from(joined).unwrap_or(HeaderValue::from_static(""))
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

    /// The policy these settings describe, or the boot's refusal naming the
    /// value it cannot take. A `*` in `origins`, `methods` or `headers` admits
    /// every value; no method at all admits what the routes serve
    /// ([`CorsPolicy::serving`]).
    pub(crate) fn policy(&self) -> Result<CorsPolicy> {
        self.reject_wildcards_with_credentials()?;
        self.reject_forbidden_methods()?;
        let origins = listed(&self.origins)
            .iter()
            .map(|origin| {
                HeaderValue::from_str(origin)
                    .with_context(|| format!("invalid origin in CORS allow-list: {origin:?}"))
            })
            .collect::<Result<_>>()?;
        let methods = if holds_wildcard(&self.methods) {
            Admitted::Every
        } else if self.methods.is_empty() {
            Admitted::Routed
        } else {
            let mut methods: Vec<Method> = Vec::new();
            for m in &self.methods {
                let method = Method::from_bytes(m.as_bytes())
                    .with_context(|| format!("invalid HTTP method in CORS config: `{m}`"))?;
                if !methods.contains(&method) {
                    methods.push(method);
                }
            }
            Admitted::Listed(methods)
        };
        let mut headers: Vec<HeaderName> = Vec::new();
        for h in listed(&self.headers) {
            let header = HeaderName::from_str(h)
                .with_context(|| format!("invalid header name in CORS allow-list: `{h}`"))?;
            if !headers.contains(&header) {
                headers.push(header);
            }
        }
        let mut exposed: Vec<HeaderName> = Vec::new();
        for h in &self.exposed_headers {
            let header = HeaderName::from_str(h)
                .with_context(|| format!("invalid header name in CORS expose-list: `{h}`"))?;
            if !exposed.contains(&header) {
                exposed.push(header);
            }
        }
        for h in FRAMEWORK_SENT {
            let header = HeaderName::from_static(h);
            if !exposed.contains(&header) {
                exposed.push(header);
            }
        }
        let max_age = self
            .max_age
            .map(|age| {
                i32::try_from(age.as_secs())
                    .context("CORS max_age overflows i32 seconds (~68 years); pick a smaller value")
            })
            .transpose()?
            .map(HeaderValue::from);
        Ok(CorsPolicy {
            origins,
            methods,
            headers,
            exposed: joined(exposed.iter().map(HeaderName::as_str)),
            credentials: self.credentials,
            max_age,
        })
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

    fn cfg(origins: &[&str]) -> HttpCors {
        HttpCors {
            origins: origins.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }
    }

    fn err_string(result: Result<CorsPolicy>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(err) => format!("{err:#}"),
        }
    }

    #[test]
    fn a_cors_policy_accepts_an_empty_config_and_a_basic_origin_list() {
        cfg(&[]).policy().expect("empty config builds");
        cfg(&["https://app.example.com"])
            .policy()
            .expect("valid config");
    }

    #[test]
    fn a_cors_policy_rejects_an_invalid_method() {
        // Spaces aren't token characters in RFC 9110 §9 — `Method::from_bytes`
        // refuses them.
        let cfg = HttpCors {
            methods: vec!["BAD METHOD".into()],
            ..cfg(&["*"])
        };
        let err = err_string(cfg.policy());
        assert!(err.contains("invalid HTTP method"), "got: {err}");
    }

    #[test]
    fn a_cors_policy_rejects_an_invalid_header_name_in_either_list() {
        for (cfg, list) in [
            (
                HttpCors {
                    headers: vec!["bad header!".into()],
                    ..cfg(&["*"])
                },
                "allow-list",
            ),
            (
                HttpCors {
                    exposed_headers: vec!["bad header!".into()],
                    ..cfg(&["*"])
                },
                "expose-list",
            ),
        ] {
            let err = err_string(cfg.policy());
            assert!(err.contains("invalid header name"), "got: {err}");
            assert!(err.contains(list), "must name the list: {err}");
        }
    }

    #[test]
    fn a_cors_policy_rejects_a_max_age_that_overflows_i32_seconds() {
        let cfg = HttpCors {
            max_age: Some(Duration::from_secs(u64::MAX)),
            ..cfg(&["*"])
        };
        let err = err_string(cfg.policy());
        assert!(err.contains("max_age overflows"), "got: {err}");
    }

    #[test]
    fn a_cors_policy_accepts_credentials_and_max_age_and_exposed_headers() {
        let cfg = HttpCors {
            origins: vec!["https://app.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            headers: vec!["content-type".into(), "x-trace-id".into()],
            exposed_headers: vec!["x-trace-id".into()],
            credentials: true,
            max_age: Some(Duration::from_secs(60 * 60)),
        };
        cfg.policy().expect("a fully-specified config builds");
    }

    #[test]
    fn credentials_refuse_a_wildcard_in_every_list_the_rule_binds() {
        type Case = (&'static str, &'static str, fn(&mut HttpCors));
        let cases: [Case; 4] = [
            ("origins", "Access-Control-Allow-Origin", |c| {
                c.origins = vec![WILDCARD.into()]
            }),
            ("headers", "Access-Control-Allow-Headers", |c| {
                c.headers = vec![WILDCARD.into()]
            }),
            ("methods", "Access-Control-Allow-Methods", |c| {
                c.methods = vec![WILDCARD.into()]
            }),
            ("exposed_headers", "Access-Control-Expose-Headers", |c| {
                c.exposed_headers = vec![WILDCARD.into()]
            }),
        ];
        for (field, header, wildcard) in cases {
            let mut cfg = HttpCors {
                credentials: true,
                ..cfg(&["https://app.example.com"])
            };
            wildcard(&mut cfg);
            let err = err_string(cfg.policy());
            assert!(err.contains(field), "must name the list: {err}");
            assert!(err.contains(header), "must name the header: {err}");
        }
    }

    #[test]
    fn a_wildcard_without_credentials_still_builds() {
        HttpCors {
            origins: vec![WILDCARD.into()],
            headers: vec![WILDCARD.into()],
            methods: vec![WILDCARD.into()],
            exposed_headers: vec![WILDCARD.into()],
            credentials: false,
            ..Default::default()
        }
        .policy()
        .expect("a wildcard policy with no credentials is legal");
    }

    const LISTED: &str = "https://app.example.com";

    /// `cfg`'s policy around an endpoint answering `served`, serving a table
    /// that routes `GET`.
    fn served(cfg: HttpCors) -> Cors<impl Endpoint> {
        cfg.policy()
            .expect("the policy builds")
            .serving(&[Method::GET], false)
            .around(crate::endpoint_fn(|_req: Request| async {
                Ok(Response::builder().body("served"))
            }))
    }

    /// A preflight from the listed origin, asking for `method` and `headers`.
    async fn preflight(
        cors: &Cors<impl Endpoint>,
        method: Option<&'static str>,
        headers: Option<&'static str>,
    ) -> Response {
        let mut request = Request::builder()
            .method(Method::OPTIONS)
            .header(ORIGIN, HeaderValue::from_static(LISTED));
        if let Some(method) = method {
            request = request.header(
                ACCESS_CONTROL_REQUEST_METHOD,
                HeaderValue::from_static(method),
            );
        }
        if let Some(headers) = headers {
            request = request.header(
                ACCESS_CONTROL_REQUEST_HEADERS,
                HeaderValue::from_static(headers),
            );
        }
        cors.call(request.finish()).await.expect("answered")
    }

    /// A preflight that names no method asks for none the policy admits.
    #[tokio::test]
    async fn a_preflight_naming_no_method_is_refused() {
        let cors = served(cfg(&[LISTED]));
        let refused = preflight(&cors, None, None).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert!(!refused.headers().contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
    }

    /// A policy listing no header admits what a preflight asks for, and says
    /// so by echoing it, or `*` when it asks for none.
    #[tokio::test]
    async fn a_policy_listing_no_header_echoes_what_the_preflight_asks_for() {
        let cors = served(cfg(&[LISTED]));
        let asked = preflight(&cors, Some("GET"), Some("x-tenant, x-trace")).await;
        assert_eq!(asked.status(), StatusCode::OK);
        assert_eq!(
            asked.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "x-tenant, x-trace"
        );
        let none = preflight(&cors, Some("GET"), None).await;
        assert_eq!(none.headers()[ACCESS_CONTROL_ALLOW_HEADERS], "*");
    }

    /// A preflight is admitted when one header it asks for is listed: the
    /// answer names the listed ones, and the browser holds the request to
    /// them.
    #[tokio::test]
    async fn a_policy_listing_headers_admits_a_preflight_asking_for_one_and_names_its_own() {
        let cors = served(HttpCors {
            headers: vec!["X-Tenant".into(), "x-trace".into()],
            ..cfg(&[LISTED])
        });
        let asked = preflight(&cors, Some("GET"), Some("x-other, x-tenant")).await;
        assert_eq!(asked.status(), StatusCode::OK);
        assert_eq!(
            asked.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "x-tenant, x-trace"
        );
        let refused = preflight(&cors, Some("GET"), Some("x-other")).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn credentials_are_said_on_a_preflight_and_on_an_answer() {
        let cors = served(HttpCors {
            credentials: true,
            ..cfg(&[LISTED])
        });
        let asked = preflight(&cors, Some("GET"), None).await;
        assert_eq!(asked.headers()[ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");
        let read = cors
            .call(
                Request::builder()
                    .header(ORIGIN, HeaderValue::from_static(LISTED))
                    .finish(),
            )
            .await
            .expect("answered");
        assert_eq!(read.headers()[ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");
        assert_eq!(read.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], LISTED);
    }

    /// A request with no `Origin` is no CORS request, and its answer still
    /// varies on the header it did not send.
    #[tokio::test]
    async fn an_answer_to_a_request_with_no_origin_varies_on_it() {
        let cors = served(cfg(&[LISTED]));
        let read = cors
            .call(Request::builder().finish())
            .await
            .expect("answered");
        assert!(!read.headers().contains_key(ACCESS_CONTROL_ALLOW_ORIGIN));
        assert_eq!(read.headers()[http::header::VARY], "origin");
        assert_eq!(read.into_body().into_string().await.unwrap(), "served");
    }
}
