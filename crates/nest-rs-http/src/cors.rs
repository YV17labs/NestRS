//! CORS settings for the HTTP transport, read from `<PREFIX>_HTTP__CORS_*` or
//! pinned as `HttpConfig.cors`, checked at boot into a [`CorsPolicy`], and
//! answered around every route by [`Cors`] (WHATWG Fetch, *CORS protocol*).

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN,
};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use nest_rs_config::{
    Bound, BoundedSetting, ConfigService, DurationBounds, Field, Floor, Namespaced,
};

use crate::endpoint::Endpoint;
use crate::error::{HttpError, ORIGIN_GRAMMAR};
use crate::origin::ListedOrigins;
use crate::problem::ProblemDetails;
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::vary::vary_on;

/// The preflight lifetime: its range, the vocabulary's key it is read under,
/// and the field that pins it.
const MAX_AGE: BoundedSetting<Option<Duration>> = BoundedSetting::new(
    DurationBounds::secs(
        Floor::Units(Bound::new(
            1,
            "a lifetime under a second caches no preflight, which leaving it off already says",
        )),
        Bound::new(
            24 * 60 * 60,
            "every browser caps a preflight's cache at a day or less (Firefox a day, Chromium \
             two hours), so a longer lifetime is a unit slip",
        ),
    ),
    Field::new("CORS_MAX_AGE", "HttpCors::max_age"),
);

/// Cross-Origin Resource Sharing policy. `origins` empty ⇒ no CORS layer
/// installed (the default). Lists are comma-separated in env vars.
///
/// The same `origins` decide which browser origins open a WebSocket gateway's
/// socket or a graphql-ws socket and call `/mcp`, which a browser never asks
/// CORS about; the CORS layer answers first and admits only what they list,
/// the server's own origin included. The list is read once, at boot, for both.
///
/// Every entry of every list is read at boot, and one the list cannot take
/// fails it, named by its list and its place, never by its value.
#[derive(Clone, Debug, Default)]
pub struct HttpCors {
    /// Allowed origins; empty ⇒ no CORS layer installed. Also the browser
    /// origins the socket upgrades and `/mcp` admit.
    ///
    /// Each entry is an origin as a browser sends it, `scheme://host[:port]`:
    /// no path (a trailing `/` included), query, fragment or userinfo, and an
    /// internationalized host in IDNA's ASCII form (`xn--…`). It is compared
    /// as an origin (RFC 6454 §5): scheme and host without case, the scheme's
    /// default port implied, so `https://App.example:443` lists what
    /// `https://app.example` does. `*` admits every origin; `null`, which any
    /// sandboxed or `data:` document sends, fails the boot.
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
    /// `<PREFIX>_HTTP__CORS_MAX_AGE_SECS`, whole seconds from 1 to a day, or `0`
    /// for none — refused outside, from the environment and from code alike.
    pub max_age: Option<Duration>,
}

/// The wildcard, as the four lists spell it.
pub(crate) const WILDCARD: &str = "*";

/// The response headers the framework sends on a route's behalf. None is
/// CORS-safelisted, so a cross-origin client reads them only when exposed.
const FRAMEWORK_SENT: [&str; 3] = ["link", "location", "retry-after"];

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

/// The forbidden method `method` names, matched without case.
fn forbidden(method: &str) -> Option<&'static str> {
    FORBIDDEN_METHODS
        .into_iter()
        .find(|forbidden| forbidden.eq_ignore_ascii_case(method))
}

/// What a policy naming no method admits: the CORS-safelisted methods, each
/// method `routed` names, and every method a page may send when an address
/// answers `every_method` itself — never a forbidden one.
fn admitted_by_default<'a>(
    routed: impl IntoIterator<Item = &'a Method>,
    every_method: bool,
) -> Vec<Method> {
    let mut admitted = match every_method {
        true => SENDABLE_METHODS.to_vec(),
        false => SAFELISTED_METHODS.to_vec(),
    };
    for method in routed {
        if forbidden(method.as_str()).is_none() && !admitted.contains(method) {
            admitted.push(method.clone());
        }
    }
    admitted
}

/// The four lists a CORS policy holds, each with the names a deployment sets
/// it by and the header it renders into.
#[derive(Clone, Copy, Debug)]
enum CorsList {
    Origins,
    Methods,
    Headers,
    ExposedHeaders,
}

impl CorsList {
    /// The [`HttpCors`] field.
    fn field(self) -> &'static str {
        match self {
            Self::Origins => "origins",
            Self::Methods => "methods",
            Self::Headers => "headers",
            Self::ExposedHeaders => "exposed_headers",
        }
    }

    /// The key `<PREFIX>_HTTP__<KEY>` reads it from: its field's name under
    /// `CORS`, the key of the `HttpConfig` field holding the policy.
    fn key(self) -> &'static str {
        match self {
            Self::Origins => "CORS_ORIGINS",
            Self::Methods => "CORS_METHODS",
            Self::Headers => "CORS_HEADERS",
            Self::ExposedHeaders => "CORS_EXPOSED_HEADERS",
        }
    }

    /// The response header the list renders into.
    fn header(self) -> &'static str {
        match self {
            Self::Origins => "Access-Control-Allow-Origin",
            Self::Methods => "Access-Control-Allow-Methods",
            Self::Headers => "Access-Control-Allow-Headers",
            Self::ExposedHeaders => "Access-Control-Expose-Headers",
        }
    }

    /// What each of its entries is.
    fn grammar(self) -> &'static str {
        match self {
            Self::Origins => ORIGIN_GRAMMAR,
            Self::Methods => "a method, a token (RFC 9110 §9.1)",
            Self::Headers | Self::ExposedHeaders => "a header name, a token (RFC 9110 §5.1)",
        }
    }

    /// The boot's refusal of entry `index` (from zero), which `is` says by
    /// kind: by its place, never its value, since a deployment's config is
    /// its data.
    fn refuse(self, index: usize, is: impl fmt::Display) -> anyhow::Error {
        anyhow::anyhow!(
            "invalid CORS config: entry {} of `{}` ({}) {is}",
            index + 1,
            self.field(),
            nest_rs_config::var_name(<crate::HttpConfig as Namespaced>::NAMESPACE, self.key()),
        )
    }
}

/// Which check of the policy a cross-origin request failed.
#[derive(Clone, Copy, Debug)]
enum CorsRefusal {
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
    origins: ListedOrigins,
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
    /// The origins the policy lists, which the browser-origin check of every
    /// upgrade and of `/mcp` reads too.
    pub(crate) fn origins(&self) -> &ListedOrigins {
        &self.origins
    }

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
    origins: ListedOrigins,
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

    /// Whether a preflight asking for the method `requested` may send it; one
    /// naming none asks for none the policy admits.
    fn admits_method(&self, requested: Option<&HeaderValue>) -> bool {
        requested
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<Method>().ok())
            .is_some_and(|method| {
                self.methods
                    .as_ref()
                    .is_none_or(|methods| methods.contains(&method))
            })
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
    /// The answer to a request carrying `origin`, before it varies. An
    /// admitted answer echoes `origin` as sent, the bytes a browser compares
    /// `Access-Control-Allow-Origin` with.
    async fn answer(&self, req: Request, origin: HeaderValue) -> Response {
        let policy = &self.policy;
        if !policy.origins.admits(&origin) {
            return CorsRefusal::Origin.answer();
        }
        if req.method() == Method::OPTIONS {
            if !policy.admits_method(req.headers().get(ACCESS_CONTROL_REQUEST_METHOD)) {
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

/// Each entry of `list`, `values`, parsed as a `T`, kept once, in the order
/// first listed; the first that does not parse is the boot's refusal, by its
/// place. `*` parses as the token it is, so an entry beside it is read too.
fn parsed_once<T: FromStr + PartialEq>(list: CorsList, values: &[String]) -> Result<Vec<T>> {
    let mut parsed = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let value = value
            .parse::<T>()
            .ok()
            .ok_or_else(|| list.refuse(index, format_args!("is not {}", list.grammar())))?;
        if !parsed.contains(&value) {
            parsed.push(value);
        }
    }
    Ok(parsed)
}

/// Whether a list holds the wildcard, which admits every value.
pub(crate) fn holds_wildcard(values: &[String]) -> bool {
    values.iter().any(|value| value == WILDCARD)
}

impl HttpCors {
    /// The entries of `list`.
    fn list(&self, list: CorsList) -> &[String] {
        match list {
            CorsList::Origins => &self.origins,
            CorsList::Methods => &self.methods,
            CorsList::Headers => &self.headers,
            CorsList::ExposedHeaders => &self.exposed_headers,
        }
    }

    /// Refuse a method no page can send: a policy listing it would claim to
    /// admit what no browser asks for.
    fn reject_forbidden_methods(&self) -> Result<()> {
        let named = self
            .methods
            .iter()
            .enumerate()
            .find_map(|(index, method)| Some((index, forbidden(method)?)));
        if let Some((index, method)) = named {
            return Err(CorsList::Methods.refuse(
                index,
                format_args!(
                    "is `{method}`, a forbidden method — WHATWG Fetch never lets a page send \
                     it, so no preflight asks for it; drop it from the list"
                ),
            ));
        }
        Ok(())
    }

    /// Refuse a `*` that credentials turn into a literal no browser honours:
    /// in every list, a `*` is a literal once the request's credentials mode is
    /// `include` — WHATWG Fetch, *CORS protocol*. `*` is a valid `tchar`, so
    /// nothing else refuses it: the header is emitted and every browser drops it.
    fn reject_wildcards_with_credentials(&self) -> Result<()> {
        if !self.credentials {
            return Ok(());
        }
        for list in [
            CorsList::Origins,
            CorsList::Headers,
            CorsList::Methods,
            CorsList::ExposedHeaders,
        ] {
            if holds_wildcard(self.list(list)) {
                anyhow::bail!(
                    "invalid CORS config: `{WILDCARD}` in `{}` with `credentials` — for a \
                     request whose credentials mode is `include`, WHATWG Fetch reads \
                     `{WILDCARD}` in `{}` as that literal value rather than as a \
                     wildcard, so the response is emitted and no browser honours it; list the \
                     values explicitly or turn `credentials` off",
                    list.field(),
                    list.header(),
                );
            }
        }
        Ok(())
    }

    /// The policy these settings describe — `None` when they list no origin,
    /// which installs no CORS layer — or the boot's refusal naming the entry
    /// it cannot take by its list and its place. A `*` in `origins`, `methods`
    /// or `headers` admits every value; no method at all admits what the
    /// routes serve ([`CorsPolicy::serving`]).
    pub(crate) fn policy(&self) -> Result<Option<CorsPolicy>> {
        self.reject_wildcards_with_credentials()?;
        self.reject_forbidden_methods()?;
        let origins = ListedOrigins::parse(&self.origins)
            .map_err(|(index, defect)| CorsList::Origins.refuse(index, defect))?;
        let methods = parsed_once::<Method>(CorsList::Methods, &self.methods)?;
        let methods = if holds_wildcard(&self.methods) {
            Admitted::Every
        } else if methods.is_empty() {
            Admitted::Routed
        } else {
            Admitted::Listed(methods)
        };
        let mut headers = parsed_once::<HeaderName>(CorsList::Headers, &self.headers)?;
        if holds_wildcard(&self.headers) {
            headers.clear();
        }
        let mut exposed =
            parsed_once::<HeaderName>(CorsList::ExposedHeaders, &self.exposed_headers)?;
        for sent in FRAMEWORK_SENT.map(HeaderName::from_static) {
            if !exposed.contains(&sent) {
                exposed.push(sent);
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
        if self.origins.is_empty() {
            return Ok(None);
        }
        Ok(Some(CorsPolicy {
            origins,
            methods,
            headers,
            exposed: joined(exposed.iter().map(HeaderName::as_str)),
            credentials: self.credentials,
            max_age,
        }))
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
        let origins = env.list(CorsList::Origins.key(), base.origins)?;
        if origins.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            origins,
            methods: env.list(CorsList::Methods.key(), base.methods)?,
            headers: env.list(CorsList::Headers.key(), base.headers)?,
            exposed_headers: env.list(CorsList::ExposedHeaders.key(), base.exposed_headers)?,
            credentials: env.flag("CORS_CREDENTIALS", base.credentials)?,
            max_age: MAX_AGE.read(env, base.max_age)?,
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
                ("CORS_MAX_AGE_SECS", "600"),
            ],
        );
        let cfg = HttpCors::from_env(&service, None)
            .expect("no error")
            .expect("Some");
        assert!(cfg.credentials);
        assert_eq!(cfg.max_age, Some(Duration::from_secs(600)));
    }

    /// A preflight's lifetime is a duration like any other: its key names the
    /// unit, `0` turns it off, and a value past a day — every browser's cap —
    /// is refused from the environment and from code alike.
    #[test]
    fn the_preflight_lifetime_is_held_to_its_bounds() {
        let read = |vars: &[(&str, &str)], base: Option<HttpCors>| {
            let mut vars = vars.to_vec();
            vars.push(("CORS_ORIGINS", "https://a.example"));
            HttpCors::from_env(&ConfigService::with_vars("http", vars), base)
        };
        let off = read(&[("CORS_MAX_AGE_SECS", "0")], None)
            .expect("`0` is off")
            .expect("a policy");
        assert_eq!(off.max_age, None);
        let refused = read(&[("CORS_MAX_AGE_SECS", "86401")], None)
            .expect_err("past a day")
            .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("http", "CORS_MAX_AGE_SECS"))
                && refused.contains("must be at most 86400 seconds"),
            "{refused}"
        );
        let pinned = read(
            &[],
            Some(HttpCors {
                max_age: Some(Duration::from_secs(86_401)),
                ..HttpCors::default()
            }),
        )
        .expect_err("a pinned lifetime past a day")
        .to_string();
        assert!(
            pinned.contains("`HttpCors::max_age` set in code"),
            "{pinned}"
        );
    }

    fn cfg(origins: &[&str]) -> HttpCors {
        HttpCors {
            origins: origins.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }
    }

    fn err_string(result: Result<Option<CorsPolicy>>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(err) => format!("{err:#}"),
        }
    }

    /// A list naming no origin is no policy, however the rest reads: no
    /// layer is installed, so nothing is admitted across origins.
    #[test]
    fn a_cors_policy_listing_no_origin_is_none_and_a_basic_origin_list_builds() {
        let none = HttpCors {
            credentials: true,
            ..cfg(&[])
        };
        assert!(none.policy().expect("it reads").is_none());
        assert!(
            cfg(&["https://app.example.com"])
                .policy()
                .expect("valid config")
                .is_some()
        );
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
            .expect("the test lists an origin")
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

    /// What no browser sends as an origin: each fails the build, named by its
    /// list and its place, and no part of it reaches the sentence.
    #[test]
    fn an_entry_that_is_no_origin_fails_the_build_by_place_never_by_value() {
        let var = nest_rs_config::var_name("http", "CORS_ORIGINS");
        for entry in [
            "https://app.example/",
            "https://app.example/path",
            "https://app.example?q=marker",
            "https://app.example#marker",
            "https://user@app.example",
            "https://é.example",
            "https://app\u{7f}.example",
            "https://app .example",
            "app.example",
            "https://",
            "https://app.example:99999",
            "https://app.example:https",
            "https://[::1",
            "null",
            "NULL",
        ] {
            let err = err_string(cfg(&["https://listed.example", entry]).policy());
            assert!(err.contains("entry 2 of `origins`"), "{entry:?}: {err}");
            assert!(err.contains(&var), "{entry:?} names its variable: {err}");
            for quoted in [
                "app.example",
                "listed.example",
                "app\u{7f}",
                "é",
                "user@",
                "/path",
                "marker",
                "99999",
                "::1",
            ] {
                assert!(!err.contains(quoted), "{entry:?} quoted: {err}");
            }
        }
    }

    /// `null` is what a sandboxed or `data:` document sends from any site:
    /// the refusal says so, and names the wildcard as the honest opening.
    #[test]
    fn null_fails_the_build_saying_why_and_what_to_write_instead() {
        let err = err_string(cfg(&["null"]).policy());
        assert!(err.contains("entry 1 of `origins`"), "{err}");
        assert!(err.contains("sandboxed"), "says why: {err}");
        assert!(err.contains("`*`"), "says what to write instead: {err}");
    }

    /// A list's key is its field's name under `CORS`, the field holding the
    /// policy, so a deployment derives every variable from the struct.
    #[test]
    fn a_lists_key_is_its_fields_name_under_cors() {
        for list in [
            CorsList::Origins,
            CorsList::Methods,
            CorsList::Headers,
            CorsList::ExposedHeaders,
        ] {
            assert_eq!(
                list.key(),
                format!("CORS_{}", list.field().to_ascii_uppercase())
            );
        }
    }

    /// A method, a header name or an exposed header name that is no token is
    /// refused by place, the same shape as an origin.
    #[test]
    fn an_entry_of_the_other_lists_is_refused_by_place_never_by_value() {
        type Case = (&'static str, &'static str, fn(&mut HttpCors));
        let cases: [Case; 3] = [
            ("methods", "CORS_METHODS", |c| {
                c.methods = vec!["GET".into(), "BAD marker".into()]
            }),
            ("headers", "CORS_HEADERS", |c| {
                c.headers = vec!["x-tenant".into(), "bad marker!".into()]
            }),
            ("exposed_headers", "CORS_EXPOSED_HEADERS", |c| {
                c.exposed_headers = vec!["x-cost".into(), "bad marker!".into()]
            }),
        ];
        for (list, key, set) in cases {
            let mut cfg = cfg(&["*"]);
            set(&mut cfg);
            let err = err_string(cfg.policy());
            assert!(err.contains(&format!("entry 2 of `{list}`")), "{err}");
            assert!(
                err.contains(&nest_rs_config::var_name("http", key)),
                "{err}"
            );
            assert!(!err.contains("marker"), "{list} quoted: {err}");
        }
    }

    /// An entry beside the wildcard is held to its list's grammar all the
    /// same: no entry is kept without being read.
    #[test]
    fn an_entry_beside_the_wildcard_is_read_too() {
        let err = err_string(cfg(&["*", "https://app.example/"]).policy());
        assert!(err.contains("entry 2 of `origins`"), "{err}");
        let err = err_string(
            HttpCors {
                headers: vec!["*".into(), "bad header!".into()],
                ..cfg(&["*"])
            }
            .policy(),
        );
        assert!(err.contains("entry 2 of `headers`"), "{err}");
    }

    /// A list read from the environment and one pinned in code are one list:
    /// the same entry is refused in the same words.
    #[test]
    fn the_environment_and_a_pinned_list_are_refused_alike() {
        let env =
            ConfigService::with_vars("http", [("CORS_ORIGINS", "https://listed.example, null")]);
        let read = HttpCors::from_env(&env, None)
            .expect("the list reads")
            .expect("origins are set");
        let pinned = cfg(&["https://listed.example", "null"]);
        let from_env = err_string(read.policy());
        assert!(from_env.contains("entry 2 of `origins`"), "{from_env}");
        assert_eq!(from_env, err_string(pinned.policy()));
    }

    /// A request from `origin`, answered under `cors`.
    async fn from_origin(cors: &Cors<impl Endpoint>, origin: &'static str) -> Response {
        cors.call(
            Request::builder()
                .header(ORIGIN, HeaderValue::from_static(origin))
                .finish(),
        )
        .await
        .expect("answered")
    }

    /// An entry is the origin it parses to (RFC 6454 §5): scheme and host
    /// without case, the default port implied; the answer echoes the
    /// request's own bytes, which the browser compares with its own.
    #[tokio::test]
    async fn an_origin_is_matched_as_the_tuple_it_parses_to_and_echoed_as_sent() {
        let cors = served(cfg(&["https://APP.example:443"]));
        for sent in ["https://app.example", "HTTPS://App.Example:443"] {
            let read = from_origin(&cors, sent).await;
            assert_eq!(read.status(), StatusCode::OK, "{sent}");
            assert_eq!(read.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], sent);
        }
        for sent in [
            "https://app.example:8443",
            "http://app.example",
            "https://app.example/",
            "null",
        ] {
            let refused = from_origin(&cors, sent).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{sent}");
        }
    }

    /// An internationalized host is listed as browsers send it, in IDNA's
    /// ASCII form.
    #[tokio::test]
    async fn an_internationalized_host_listed_in_its_ascii_form_admits_what_browsers_send() {
        let cors = served(cfg(&["https://xn--9ca.example"]));
        let read = from_origin(&cors, "https://xn--9ca.example").await;
        assert_eq!(read.status(), StatusCode::OK);
        assert_eq!(
            read.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://xn--9ca.example"
        );
    }

    /// `null` admits nothing a list names; the wildcard, which says what it
    /// opens, admits it as it admits every origin.
    #[tokio::test]
    async fn null_is_admitted_by_the_wildcard_alone() {
        let listed = served(cfg(&[LISTED]));
        assert_eq!(
            from_origin(&listed, "null").await.status(),
            StatusCode::FORBIDDEN
        );
        let every = served(cfg(&[WILDCARD]));
        assert_eq!(from_origin(&every, "null").await.status(), StatusCode::OK);
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
