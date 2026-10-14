//! How a request says which API version it wants.
//!
//! `#[controller(version = "1")]` declares a version; a strategy selects it, and
//! all three resolve to the same mounted path, [`version_path`]:
//!
//! | Strategy | The caller writes | Resolved |
//! |---|---|---|
//! | [`Uri`](ApiVersioning::Uri) | `GET /v2/users` | at routing |
//! | [`Header`](ApiVersioning::Header) | `GET /users` + `X-API-Version: 2` | per request |
//! | [`MediaType`](ApiVersioning::MediaType) | `GET /users` + `Accept: application/json; version=2` | per request |
//!
//! The last two rewrite the request's path in front of routing.

use std::str::FromStr;
use std::sync::Arc;

use http::uri::PathAndQuery;
use http::{HeaderName, StatusCode, Uri, header};
use nest_rs_core::{Container, Discovery};

use crate::endpoint::Endpoint;
use crate::error::{HttpError, Result};
use crate::problem::ProblemDetails;
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::route_template::{segment_parameter, strip_literal};
use crate::version_path;

/// The media-type parameter the [`MediaType`](ApiVersioning::MediaType)
/// strategy reads (`Accept: application/json; version=2`).
pub const MEDIA_TYPE_PARAM: &str = "version";

/// The default header the [`Header`](ApiVersioning::Header) strategy reads.
pub const DEFAULT_VERSION_HEADER: &str = "x-api-version";

/// The longest version token accepted from a request.
const MAX_VERSION_LEN: usize = 32;

/// How a caller selects an API version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApiVersioning {
    /// `/v2/users` — the version is part of the path. The default.
    #[default]
    Uri,
    /// `X-API-Version: 2` on an unversioned path.
    Header,
    /// `Accept: application/json; version=2` on an unversioned path.
    MediaType,
}

impl ApiVersioning {
    /// The spelling used in `<PREFIX>_HTTP__VERSIONING`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uri => "uri",
            Self::Header => "header",
            Self::MediaType => "media_type",
        }
    }
}

impl FromStr for ApiVersioning {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "uri" | "url" | "path" => Ok(Self::Uri),
            "header" => Ok(Self::Header),
            "media_type" | "accept" => Ok(Self::MediaType),
            other => Err(format!(
                "unknown API versioning strategy {other:?} — expected `uri`, `header` or \
                 `media_type`",
            )),
        }
    }
}

/// Reads the requested version off a request and folds it into the path.
///
/// It rewrites only paths a versioned route serves, so a default version never
/// sends `/graphql` or `/health` to `/v1/graphql`.
#[derive(Clone, Debug)]
pub(crate) struct VersionSelector {
    strategy: ApiVersioning,
    header: HeaderName,
    default_version: Option<String>,
    /// Every **route** a versioned controller mounts, as mounted
    /// (`/v1/posts`, `/v1/posts/{id}`).
    versioned_routes: Arc<[String]>,
    /// The templates self-mounted endpoints registered (`/graphql`, `/mcp`, a
    /// gateway, an entry below one), served as sent whatever version a caller
    /// states.
    self_mounts: Arc<[String]>,
    /// The routes unversioned controllers mount: neutral against a default
    /// version, not against a stated one.
    unversioned_routes: Arc<[String]>,
    /// The versions the app declares, to answer `404` when a stated version
    /// does not serve an address another version does.
    versions: Arc<[String]>,
    /// The global prefix every address is registered under, which the version
    /// follows: `/api/users` resolves to `/api/v2/users`.
    prefix: Option<Arc<str>>,
}

impl VersionSelector {
    /// Build a selector. `header` names the header the
    /// [`Header`](ApiVersioning::Header) strategy reads; `default_version` is
    /// what a request that states none is served, and `None` leaves such a
    /// request on the unversioned routes. The routes are learned when
    /// [`HttpTransport`](crate::HttpTransport) mounts the app's controllers.
    pub(crate) fn new(
        strategy: ApiVersioning,
        header: HeaderName,
        default_version: Option<String>,
    ) -> Self {
        Self {
            strategy,
            header,
            default_version,
            versioned_routes: Arc::from(Vec::new()),
            self_mounts: Arc::from(Vec::new()),
            unversioned_routes: Arc::from(Vec::new()),
            versions: Arc::from(Vec::new()),
            prefix: None,
        }
    }

    /// Teach the selector the app's shape: the routes that carry a version as
    /// mounted (`/v1/posts/{id}`), every address answered without one, and the
    /// versions declared — each address without the global prefix.
    pub(crate) fn with_routes(
        mut self,
        versioned: Vec<String>,
        self_mounts: Vec<String>,
        unversioned: Vec<String>,
        versions: Vec<String>,
    ) -> Self {
        self.versioned_routes = Arc::from(versioned);
        self.self_mounts = Arc::from(self_mounts);
        self.unversioned_routes = Arc::from(unversioned);
        self.versions = Arc::from(versions);
        self
    }

    /// The selector for addresses registered under the global `prefix`, a
    /// canonical literal path.
    pub(crate) fn under(mut self, prefix: Option<&str>) -> Self {
        self.prefix = prefix.map(Arc::from);
        self
    }

    /// `true` when no controller declares a version; the transport then skips
    /// the wrap.
    pub(crate) fn is_inert(&self) -> bool {
        self.versioned_routes.is_empty()
    }

    /// `true` when this selector rewrites requests — i.e. anything but the
    /// URI strategy, which routing already handles.
    pub(crate) fn rewrites(&self) -> bool {
        self.strategy != ApiVersioning::Uri
    }

    /// The version a request that states none is served, if the deployment
    /// names one.
    pub(crate) fn default_version(&self) -> Option<&str> {
        self.default_version.as_deref()
    }

    /// The version this request asks for, before validation.
    fn requested<'a>(&self, req: &'a Request) -> Requested<'a> {
        match self.strategy {
            ApiVersioning::Uri => Requested::Absent,
            ApiVersioning::Header => match req.headers().get(&self.header) {
                None => Requested::Absent,
                // httparse admits `0x80..=0xFF` in a value and `to_str` refuses
                // it: a stated value, so malformed, never absent.
                Some(raw) => match raw.to_str() {
                    Ok(value) => Requested::Stated(value),
                    Err(_) => Requested::Malformed,
                },
            },
            ApiVersioning::MediaType => accept_version(req),
        }
    }

    fn is_versioned(&self, path: &str) -> bool {
        self.versioned_routes
            .iter()
            .any(|route| route_matches(path, route))
    }

    fn is_self_mount(&self, path: &str) -> bool {
        self.self_mounts
            .iter()
            .any(|route| route_matches(path, route))
    }

    fn is_unversioned(&self, path: &str) -> bool {
        self.unversioned_routes
            .iter()
            .any(|route| route_matches(path, route))
    }

    /// Whether some declared version serves `path`. Allocates; called only on
    /// the refusal path.
    fn has_any_version(&self, path: &str) -> bool {
        self.versions
            .iter()
            .any(|version| self.is_versioned(&version_path(Some(version), path)))
    }
}

/// Every version the app's mounted controllers declare, sorted and deduplicated.
pub fn declared_versions(container: &Container) -> Vec<String> {
    let mut versions: Vec<String> = Discovery::new(container)
        .meta::<crate::HttpControllerMeta>()
        .iter()
        .flat_map(|d| d.meta.versions)
        .map(|v| (*v).to_owned())
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

/// Does `path` address the mounted route `pattern`, a template?
///
/// Segment-wise, over every form the route grammar spells: `{name}` takes one
/// segment, `{*rest}` everything left including nothing, a parameter may
/// follow literal text in its segment (`/@{handle}`, `/report-{id}`), and `{{`
/// is a literal brace.
///
/// Loose on purpose, since the router decides last: a false match costs a
/// `404`, a false non-match serves another controller's body. Runs on every
/// request, so it does not allocate.
pub(crate) fn route_matches(path: &str, pattern: &str) -> bool {
    let mut segments = path.split('/');
    let mut expected = pattern.split('/');
    loop {
        let pattern = expected.next().map(|pat| (pat, segment_parameter(pat)));
        match (segments.next(), pattern) {
            (None, None) => return true,
            // A catch-all also answers an empty tail (`/cat/`).
            (segment, Some((pat, Some((at, true))))) => {
                return at == 0 || segment.is_some_and(|s| strip_literal(s, &pat[..at]).is_some());
            }
            (Some(segment), Some((pat, parameter))) => {
                if !segment_matches(segment, pat, parameter.map(|(at, _)| at)) {
                    return false;
                }
            }
            // A trailing `/` names the same address.
            (Some(segment), None) => {
                if !segment.is_empty() {
                    return false;
                }
            }
            (None, Some(_)) => return false,
        }
    }
}

/// One path segment against one template segment whose `{name}` opens at
/// `parameter`. A `{name}` matches any non-empty text; a literal before it
/// must still match, so `/@{handle}` accepts `@bob` and refuses `bob`.
fn segment_matches(segment: &str, pattern: &str, parameter: Option<usize>) -> bool {
    match parameter {
        None => strip_literal(segment, pattern) == Some(""),
        Some(at) => strip_literal(segment, &pattern[..at]).is_some_and(|rest| !rest.is_empty()),
    }
}

/// What a request said about the version it wants.
///
/// [`Requested::Absent`] and [`Requested::Malformed`] stay apart: collapsing them
/// serves an undecodable header the default version at `200`.
enum Requested<'a> {
    /// Nothing was stated, so a deployment default may apply.
    Absent,
    /// This was stated. Still to be validated as a version token.
    Stated(&'a str),
    /// Something was stated that is not text at all.
    Malformed,
}

/// The `version=` parameter of the first media range in `Accept` carrying one.
fn accept_version(req: &Request) -> Requested<'_> {
    let Some(accept) = req.headers().get(header::ACCEPT) else {
        return Requested::Absent;
    };
    let Ok(accept) = accept.to_str() else {
        return Requested::Malformed;
    };
    accept
        .split(',')
        .find_map(|range| {
            range.split(';').skip(1).find_map(|param| {
                let (name, value) = param.split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case(MEDIA_TYPE_PARAM)
                    .then(|| value.trim().trim_matches('"'))
            })
        })
        // An ordinary `Accept` with no `version=` asked for nothing.
        .map_or(Requested::Absent, Requested::Stated)
}

/// One refusal for both checks, so a caller cannot tell which one refused it.
fn malformed_version() -> HttpError {
    HttpError::from_response(
        ProblemDetails::bad_request()
            .with_detail("malformed API version")
            .into_response(),
    )
}

/// A version token is spliced into a URL path: bare alphanumerics, `.` and `-`,
/// bounded length.
///
/// A copy of `nest_rs_codegen::versioning`'s rule (that crate pulls `syn`), held
/// equal by `the_wire_grammar_matches_the_declared_grammar`.
fn is_valid_version(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= MAX_VERSION_LEN
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Rewrites a request's path from the version it asks for, then routes it.
/// Sits inside the global prefix, so it sees the path controllers mount at.
pub(crate) struct VersionedEndpoint<E> {
    inner: E,
    selector: VersionSelector,
}

impl<E> VersionedEndpoint<E> {
    pub(crate) fn new(inner: E, selector: VersionSelector) -> Self {
        Self { inner, selector }
    }

    /// The path this request should be routed at, or `None` to route it as
    /// sent; `Err` is the `404` or `400` refusal.
    fn resolve(&self, req: &Request) -> Result<Option<String>> {
        let sent = req.uri().path();
        // Every address sits under the global prefix, and the version follows it.
        let (prefix, path) = match self.selector.prefix.as_deref() {
            None => ("", sent),
            Some(prefix) => match sent.strip_prefix(prefix) {
                Some("") => (prefix, "/"),
                Some(rest) if rest.starts_with('/') => (prefix, rest),
                _ => return Ok(None),
            },
        };
        // Before the URI-form refusal: `#[gateway(version = "1")]` self-mounts
        // at `/v1/ws`.
        if self.selector.is_self_mount(path) {
            return Ok(None);
        }

        // The mounted versioned routes are the URI form, whatever a version's spelling.
        if self.selector.is_versioned(path) {
            tracing::debug!(
                target: crate::target::HTTP,
                path = path,
                strategy = self.selector.strategy.as_str(),
                "refused a URI-versioned path under a non-URI versioning strategy",
            );
            return Err(HttpError::from_status(StatusCode::NOT_FOUND));
        }

        // `debug`: a client's own line, which the access line already records as
        // its `400`.
        let (version, stated) = match self.selector.requested(req) {
            Requested::Stated(raw) if is_valid_version(raw) => (Some(raw), true),
            Requested::Stated(raw) => {
                tracing::debug!(
                    target: crate::target::HTTP,
                    strategy = self.selector.strategy.as_str(),
                    length = raw.len(),
                    "rejected a malformed API version",
                );
                return Err(malformed_version());
            }
            Requested::Malformed => {
                tracing::debug!(
                    target: crate::target::HTTP,
                    strategy = self.selector.strategy.as_str(),
                    reason = "not valid text",
                    "rejected a malformed API version",
                );
                return Err(malformed_version());
            }
            Requested::Absent => (self.selector.default_version(), false),
        };

        // A default never moves an unversioned address; a stated version does.
        if !stated && self.selector.is_unversioned(path) {
            return Ok(None);
        }

        let Some(version) = version else {
            return Ok(None);
        };
        let candidate = version_path(Some(version), path);
        if self.selector.is_versioned(&candidate) {
            return Ok(Some(match prefix {
                "" => candidate,
                prefix => format!("{prefix}{candidate}"),
            }));
        }
        if stated && self.selector.has_any_version(path) {
            // Falling through would answer with another version's body.
            tracing::debug!(
                target: crate::target::HTTP,
                path = path,
                "no route serves the requested API version",
            );
            return Err(HttpError::from_status(StatusCode::NOT_FOUND));
        }
        Ok(None)
    }
}

impl<E: Endpoint> Endpoint for VersionedEndpoint<E> {
    async fn call(&self, mut req: Request) -> Result<Response> {
        // In front of every request: only a rewrite may allocate.
        if let Some(path) = self.resolve(&req)? {
            rewrite_path(&mut req, &path)?;
        }
        self.inner.call(req).await
    }
}

/// Swap the request's path, keeping its query. `original_uri` is untouched, so
/// the access log and every `#[meta]` reader still see what the client sent.
#[expect(
    clippy::map_err_ignore,
    reason = "the path was already accepted by the router; the refusal is the whole answer"
)]
fn rewrite_path(req: &mut Request, path: &str) -> Result<()> {
    let uri = req.uri().clone();
    let mut parts = uri.into_parts();
    let query = parts
        .path_and_query
        .as_ref()
        .and_then(|pq| pq.query())
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    parts.path_and_query = Some(PathAndQuery::try_from(format!("{path}{query}")).map_err(
        |_| {
            // Unreachable: a router-parsed path plus a validated version.
            HttpError::from_status(StatusCode::BAD_REQUEST)
        },
    )?);
    *req.uri_mut() = Uri::from_parts(parts).map_err(|_| {
        HttpError::from_response(
            ProblemDetails::bad_request()
                .with_detail("could not resolve the API version")
                .into_response(),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use http::HeaderValue;

    use super::*;
    use crate::endpoint::{BoxEndpoint, endpoint_fn};

    /// An endpoint answering each path in `routes` with its body, `404`
    /// elsewhere, and echoing the query under `?`.
    fn table(routes: &'static [(&'static str, &'static str)]) -> BoxEndpoint {
        BoxEndpoint::new(endpoint_fn(move |req: Request| async move {
            let path = req.uri().path();
            match routes.iter().find(|(at, _)| *at == path) {
                Some((_, "?")) => {
                    Ok(Response::builder().body(req.uri().query().unwrap_or_default().to_owned()))
                }
                Some((_, body)) => Ok(Response::builder().body(*body)),
                None => Err(HttpError::from_status(StatusCode::NOT_FOUND)),
            }
        }))
    }

    /// `path` asked of `endpoint`, with `name: value` when given: the status
    /// and the body.
    async fn ask(
        endpoint: &impl Endpoint,
        path: &str,
        header: Option<(HeaderName, &str)>,
    ) -> (StatusCode, String) {
        let mut req = Request::builder().uri(path.parse().unwrap());
        if let Some((name, value)) = header {
            req = req.header(name, HeaderValue::from_str(value).unwrap());
        }
        match endpoint.call(req.finish()).await {
            Ok(resp) => (resp.status(), resp.into_body().into_string().await.unwrap()),
            Err(err) => (err.status(), String::new()),
        }
    }

    fn version(value: &str) -> Option<(HeaderName, &str)> {
        Some((HeaderName::from_static(DEFAULT_VERSION_HEADER), value))
    }

    fn accept(value: &str) -> Option<(HeaderName, &str)> {
        Some((header::ACCEPT, value))
    }

    fn app(
        strategy: ApiVersioning,
        default_version: Option<&str>,
    ) -> VersionedEndpoint<BoxEndpoint> {
        let route = table(&[
            ("/v1/users", "one"),
            ("/v2/users", "two"),
            ("/users", "none"),
        ]);
        VersionedEndpoint::new(
            route,
            VersionSelector::new(
                strategy,
                HeaderName::from_static(DEFAULT_VERSION_HEADER),
                default_version.map(str::to_owned),
            )
            .with_routes(
                vec!["/v1/users".into(), "/v2/users".into()],
                vec![],
                vec!["/users".into()],
                vec!["1".into(), "2".into()],
            ),
        )
    }

    #[test]
    fn the_matcher_reads_every_segment_form_the_grammar_spells() {
        assert!(route_matches("/posts/abc", "/posts/{id}"));
        // A literal and a parameter in one segment.
        assert!(route_matches("/mix/@bob", "/mix/@{handle}"));
        assert!(!route_matches("/mix/bob", "/mix/@{handle}"));
        assert!(!route_matches("/mix/@", "/mix/@{handle}"));
        assert!(route_matches("/r/report-7", "/r/report-{id}"));
        // A parameter is one segment, not a tail.
        assert!(!route_matches("/probe/archive/7", "/probe/{id}"));
        assert!(route_matches("/cat/a/b", "/cat/{*rest}"));
        assert!(route_matches("/cat/", "/cat/{*rest}"));
        assert!(route_matches("/cat/x.gz", "/cat/x{*rest}"));
        assert!(!route_matches("/cat/y.gz", "/cat/x{*rest}"));
        // A literal brace is the brace a request carries.
        assert!(route_matches("/b/{x}", "/b/{{x}}"));
        assert!(!route_matches("/b/x", "/b/{{x}}"));
        assert!(route_matches("/users/", "/users"));
        assert!(!route_matches("/users/1", "/users"));
        assert!(!route_matches("/postsy", "/posts"));
    }

    #[test]
    fn a_selector_with_nothing_versioned_reports_itself_inert() {
        let bare = VersionSelector::new(
            ApiVersioning::Header,
            HeaderName::from_static(DEFAULT_VERSION_HEADER),
            Some("1".into()),
        );
        assert!(
            bare.is_inert(),
            "a strategy alone versions nothing — only a controller does",
        );
        assert!(
            !bare
                .clone()
                .with_routes(
                    vec!["/v1/users".into()],
                    vec![],
                    vec!["/vendors".into()],
                    vec!["1".into()],
                )
                .is_inert(),
        );
    }

    #[test]
    fn route_matching_follows_the_router_not_the_prefix() {
        assert!(route_matches("/ping", "/ping"));
        assert!(route_matches("/posts/abc", "/posts/{id}"));
        assert!(route_matches("/files/a/b/c", "/files/{*rest}"));
        assert!(!route_matches("/postsy", "/posts"));
        assert!(
            !route_matches("/posts/drafts", "/posts"),
            "a nested controller is not the versioned one above it",
        );
        assert!(
            route_matches("/root-ping", "/root-ping"),
            "a root-mounted controller's routes are ordinary routes",
        );
        assert!(
            !route_matches("/posts", "/posts/{id}"),
            "a parameter segment is required, not optional",
        );
    }

    #[test]
    fn strategies_parse_from_their_documented_spellings() {
        assert_eq!("uri".parse(), Ok(ApiVersioning::Uri));
        assert_eq!("HEADER".parse(), Ok(ApiVersioning::Header));
        assert_eq!("media-type".parse(), Ok(ApiVersioning::MediaType));
        assert_eq!("media_type".parse(), Ok(ApiVersioning::MediaType));
        let err = "v2".parse::<ApiVersioning>().expect_err("unknown strategy");
        assert!(err.contains("media_type"), "names the options: {err}");
    }

    #[tokio::test]
    async fn a_header_selects_the_version() {
        let ep = app(ApiVersioning::Header, None);
        assert_eq!(
            ask(&ep, "/users", version("2")).await,
            (StatusCode::OK, "two".into())
        );
    }

    #[tokio::test]
    async fn a_media_type_parameter_selects_the_version() {
        let ep = app(ApiVersioning::MediaType, None);
        assert_eq!(
            ask(&ep, "/users", accept("application/json; version=1")).await,
            (StatusCode::OK, "one".into())
        );
    }

    #[tokio::test]
    async fn a_quoted_media_type_parameter_is_read_the_same() {
        let ep = app(ApiVersioning::MediaType, None);
        assert_eq!(
            ask(
                &ep,
                "/users",
                accept("text/html, application/json;version=\"2\"")
            )
            .await,
            (StatusCode::OK, "two".into())
        );
    }

    #[tokio::test]
    async fn a_default_never_moves_an_address_that_already_answers_without_one() {
        for default in [Some("1"), None] {
            let ep = app(ApiVersioning::Header, default);
            assert_eq!(
                ask(&ep, "/users", None).await,
                (StatusCode::OK, "none".into())
            );
        }

        let ep = app(ApiVersioning::Header, Some("1"));
        assert_eq!(
            ask(&ep, "/users", version("2")).await,
            (StatusCode::OK, "two".into())
        );
    }

    #[tokio::test]
    async fn the_query_string_survives_the_rewrite() {
        let ep = VersionedEndpoint::new(
            table(&[("/v2/users", "?")]),
            VersionSelector::new(
                ApiVersioning::Header,
                HeaderName::from_static(DEFAULT_VERSION_HEADER),
                None,
            )
            .with_routes(vec!["/v2/users".into()], vec![], vec![], vec!["2".into()]),
        );
        assert_eq!(
            ask(&ep, "/users?first=10&after=abc", version("2")).await,
            (StatusCode::OK, "first=10&after=abc".into())
        );
    }

    #[tokio::test]
    async fn a_version_that_could_reach_another_path_is_refused() {
        let ep = app(ApiVersioning::Header, None);
        for probe in [
            "../admin",
            "1/../../etc",
            "1%2f2",
            "",
            "a".repeat(64).as_str(),
        ] {
            assert_eq!(
                ask(&ep, "/users", version(probe)).await.0,
                StatusCode::BAD_REQUEST,
                "version {probe:?} must be refused",
            );
        }
    }

    #[tokio::test]
    async fn a_uri_versioned_path_is_not_a_second_address_under_another_strategy() {
        let ep = app(ApiVersioning::Header, None);
        assert_eq!(ask(&ep, "/v2/users", None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_uri_form_is_refused_whatever_the_version_is_spelled_like() {
        let ep = VersionedEndpoint::new(
            table(&[("/v2024-08-11/users", "two")]),
            VersionSelector::new(
                ApiVersioning::Header,
                HeaderName::from_static(DEFAULT_VERSION_HEADER),
                None,
            )
            .with_routes(
                vec!["/v2024-08-11/users".into()],
                vec![],
                vec![],
                vec!["2024-08-11".into()],
            ),
        );
        assert_eq!(
            ask(&ep, "/v2024-08-11/users", None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ask(&ep, "/users", version("2024-08-11")).await,
            (StatusCode::OK, "two".into())
        );
    }

    #[tokio::test]
    async fn a_path_that_merely_starts_with_v_is_left_alone() {
        let ep = VersionedEndpoint::new(
            table(&[("/vendors", "none")]),
            VersionSelector::new(
                ApiVersioning::Header,
                HeaderName::from_static(DEFAULT_VERSION_HEADER),
                Some("1".into()),
            )
            .with_routes(
                vec!["/v1/users".into()],
                vec![],
                vec!["/vendors".into()],
                vec!["1".into()],
            ),
        );
        assert_eq!(
            ask(&ep, "/vendors", None).await,
            (StatusCode::OK, "none".into())
        );
    }

    /// The prefix is registered with every address, so the version follows it
    /// and a path outside it is routed as sent.
    #[tokio::test]
    async fn under_a_global_prefix_the_version_follows_the_prefix() {
        let ep = VersionedEndpoint::new(
            table(&[
                ("/api/v1/users", "one"),
                ("/api/v2/users", "two"),
                ("/api/v1", "root one"),
                ("/users", "outside"),
            ]),
            VersionSelector::new(
                ApiVersioning::Header,
                HeaderName::from_static(DEFAULT_VERSION_HEADER),
                Some("1".into()),
            )
            .with_routes(
                vec!["/v1/users".into(), "/v2/users".into(), "/v1".into()],
                vec![],
                vec![],
                vec!["1".into(), "2".into()],
            )
            .under(Some("/api")),
        );
        assert_eq!(
            ask(&ep, "/api/users", version("2")).await,
            (StatusCode::OK, "two".into())
        );
        assert_eq!(
            ask(&ep, "/api/users", None).await,
            (StatusCode::OK, "one".into()),
            "the default follows the prefix too"
        );
        assert_eq!(
            ask(&ep, "/api", None).await,
            (StatusCode::OK, "root one".into())
        );
        assert_eq!(
            ask(&ep, "/api/v2/users", None).await.0,
            StatusCode::NOT_FOUND,
            "the URI form under the prefix is no second address"
        );
        assert_eq!(
            ask(&ep, "/users", version("2")).await,
            (StatusCode::OK, "outside".into()),
            "a path outside the prefix is routed as sent"
        );
        assert_eq!(
            ask(&ep, "/apiary/users", version("2")).await.0,
            StatusCode::NOT_FOUND,
            "a shared spelling is not the prefix's segment"
        );
    }

    /// Holds `is_valid_version` equal to `nest_rs_codegen`'s, which this crate
    /// cannot depend on outside its tests.
    #[test]
    fn the_wire_grammar_matches_the_declared_grammar() {
        assert_eq!(
            MAX_VERSION_LEN,
            nest_rs_codegen::versioning::MAX_VERSION_LEN
        );

        // Over the whole ASCII range: a table passes for every byte it omits.
        for byte in 0u8..=0x7f {
            let case = (byte as char).to_string();
            assert_eq!(
                is_valid_version(&case),
                nest_rs_codegen::versioning::is_valid_version(&case),
                "the two halves disagree about the character {byte:#04x}",
            );
        }
        for len in [
            0,
            1,
            MAX_VERSION_LEN - 1,
            MAX_VERSION_LEN,
            MAX_VERSION_LEN + 1,
        ] {
            let case = "a".repeat(len);
            assert_eq!(
                is_valid_version(&case),
                nest_rs_codegen::versioning::is_valid_version(&case),
                "the two halves disagree about a {len}-character version",
            );
        }
        for case in [
            "1",
            "2",
            "2024-08-11",
            "1.0",
            "v1",
            "1/2",
            "1 2",
            "../admin",
            "1%2f2",
            "\u{e9}",
            "1\u{0}",
        ] {
            assert_eq!(
                is_valid_version(case),
                nest_rs_codegen::versioning::is_valid_version(case),
                "the two halves disagree about {case:?}",
            );
        }
    }
}
