//! How a request says which API version it wants.
//!
//! `#[controller(version = "1")]` stays the one place a version is *declared*;
//! this module decides how a caller *selects* one. All three strategies resolve
//! to the same mounted path — [`version_path`](crate::version_path) — so the
//! served, logged and documented routes cannot drift apart:
//!
//! | Strategy | The caller writes | Resolved |
//! |---|---|---|
//! | [`Uri`](ApiVersioning::Uri) | `GET /v2/users` | at routing |
//! | [`Header`](ApiVersioning::Header) | `GET /users` + `X-API-Version: 2` | per request |
//! | [`MediaType`](ApiVersioning::MediaType) | `GET /users` + `Accept: application/json; version=2` | per request |
//!
//! The last two are a **rewrite in front of routing**: the version is read off
//! the request, validated, and folded into the path the controller already
//! mounts at. One routing table, one source of truth, and a strategy change
//! costs an app no code.

use std::str::FromStr;
use std::sync::Arc;

use nest_rs_core::{Container, DiscoveryService};

use poem::http::uri::PathAndQuery;
use poem::http::{HeaderName, StatusCode, Uri, header};
use poem::{Endpoint, Error, IntoResponse, Request, Response, Result};

use crate::version_path;

/// The media-type parameter the [`MediaType`](ApiVersioning::MediaType)
/// strategy reads (`Accept: application/json; version=2`). A constant, not a
/// knob: it is the convention every API that uses this strategy already writes,
/// and a second spelling would only let two deployments disagree.
pub const MEDIA_TYPE_PARAM: &str = "version";

/// The default header the [`Header`](ApiVersioning::Header) strategy reads.
pub const DEFAULT_VERSION_HEADER: &str = "x-api-version";

/// The longest version token accepted from a request. Versions are `1`, `2`,
/// `2024-08-11` — anything longer is a caller probing, not an API version.
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
    /// The spelling used in `NESTRS_HTTP__VERSIONING`.
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
/// It also knows **which paths are versioned**, and that is not a refinement:
/// a rewrite that fired on every path would send `/graphql`, `/mcp` and
/// `/health` to `/v1/graphql` the moment a deployment set a default version,
/// and a self-mounted endpoint has no version to be rewritten to.
#[derive(Clone, Debug)]
pub struct VersionSelector {
    strategy: ApiVersioning,
    header: HeaderName,
    default_version: Option<String>,
    /// Every **route** a versioned controller mounts, as mounted
    /// (`/v1/posts`, `/v1/posts/:id`).
    versioned_routes: Arc<[String]>,
    /// The paths self-mounted endpoints own — `/graphql`, `/mcp`, `/api-json`,
    /// a WebSocket gateway. **Absolutely neutral**: served as sent whatever a
    /// caller states, because a self-mount owns its path outright (the boot
    /// check makes it exclusive) and has no version to be rewritten to. Without
    /// this, a versioned `#[controller(path = "/")]` with a catch-all route
    /// swallowed every one of them.
    self_mounts: Arc<[String]>,
    /// The routes unversioned controllers mount.
    ///
    /// Neutral against a **default** version — a deployment-wide default must
    /// never rewrite a controller out from under a caller who asked for nothing
    /// — but not against a **stated** one. That precedence is the point: an
    /// explicit request is the strongest signal a caller can send, so it beats
    /// an unversioned neighbour at the same address; a default is the weakest,
    /// so it yields. Collapsing the two made `#[controller(version = …)]`
    /// unreachable under a non-URI strategy whenever an unversioned route
    /// shared its address.
    unversioned_routes: Arc<[String]>,
    /// The versions the app declares, for the one question the two lists above
    /// cannot answer on their own: a caller named a version that does not serve
    /// this address — does *another* one? Yes ⇒ `404`, because answering with a
    /// different version's body is the silent failure the whole design exists to
    /// prevent. No ⇒ the address is simply not versioned, and the router decides.
    versions: Arc<[String]>,
}

impl VersionSelector {
    /// Build a selector. `header` names the header the
    /// [`Header`](ApiVersioning::Header) strategy reads; `default_version` is
    /// what a request that states none is served, and `None` leaves such a
    /// request on the unversioned routes.
    ///
    /// The versioned prefixes are learned at
    /// [`configure`](crate::HttpTransport) time, from the controllers the app
    /// actually mounts.
    pub fn new(
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
        }
    }

    /// Teach the selector the app's shape: the routes that carry a version as
    /// mounted (`/v1/posts/:id`), every address answered without one, and the
    /// versions declared.
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

    /// `true` when no controller declares a version, so this selector can never
    /// change an outcome. The transport reads it and skips the wrap entirely —
    /// an endpoint that only ever passes the request through still costs a
    /// routing layer per request.
    pub(crate) fn is_inert(&self) -> bool {
        self.versioned_routes.is_empty()
    }

    /// `true` when this selector rewrites requests — i.e. anything but the
    /// URI strategy, which routing already handles.
    pub(crate) fn rewrites(&self) -> bool {
        self.strategy != ApiVersioning::Uri
    }

    /// The version a request that states none is served, if the deployment
    /// names one. Read at boot to check it against what the app declares.
    pub(crate) fn default_version(&self) -> Option<&str> {
        self.default_version.as_deref()
    }

    /// The version this request asks for, before validation.
    fn requested<'a>(&self, req: &'a Request) -> Requested<'a> {
        match self.strategy {
            ApiVersioning::Uri => Requested::Absent,
            ApiVersioning::Header => match req.headers().get(&self.header) {
                None => Requested::Absent,
                // A header value may legally carry any byte in `0x80..=0xFF`
                // (httparse's `HEADER_VALUE_MAP`), so hyper delivers one and
                // `to_str` refuses it. The caller *stated* something; that it is
                // not text is a malformed statement, never a silent absence.
                Some(raw) => match raw.to_str() {
                    Ok(value) => Requested::Stated(value),
                    Err(_) => Requested::Malformed,
                },
            },
            ApiVersioning::MediaType => accept_version(req),
        }
    }

    /// Whether a versioned route answers at `path` as mounted (`/v1/posts`).
    fn is_versioned(&self, path: &str) -> bool {
        self.versioned_routes
            .iter()
            .any(|route| route_matches(path, route))
    }

    /// Whether a self-mounted endpoint owns `path`.
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

    /// Whether **some** declared version serves `path`. Only consulted when a
    /// caller named one that does not, to tell "you asked for the wrong
    /// version" (`404`) from "this address has no versions" (let the router
    /// answer). Allocates, and only on that error path.
    fn has_any_version(&self, path: &str) -> bool {
        self.versions
            .iter()
            .any(|version| self.is_versioned(&version_path(Some(version), path)))
    }
}

/// Every version the app's mounted controllers declare, sorted and deduplicated.
///
/// One implementation, because two consumers ask the same question for opposite
/// reasons: the transport refuses a `DEFAULT_VERSION` that names none of these,
/// and the OpenAPI module publishes a document per entry. Two walks of the same
/// metadata would eventually disagree about what "declared" means.
pub fn declared_versions(container: &Container) -> Vec<String> {
    let mut versions: Vec<String> = DiscoveryService::new(container)
        .meta::<crate::HttpControllerMeta>()
        .iter()
        .flat_map(|d| d.meta.versions)
        .map(|v| (*v).to_owned())
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

/// Does `path` address the mounted route `pattern`?
///
/// Segment-wise, and aware of the forms poem's router parses: `:name` takes one
/// segment, `<regex>` takes one segment, `*rest` takes everything left including
/// nothing, and a segment may mix a literal with a parameter (`/@:handle`,
/// `/report-:id`) — which is how a handle or a slug is written.
///
/// **It answers loosely on purpose, and that is only safe because poem always
/// decides last.** Every outcome this feeds ends with the router: a match sends
/// the request to a rewritten path the router must still recognise, and a
/// non-match sends it on as written. So a false *match* costs a `404` the
/// request was heading for anyway, while a false *non-match* once served one
/// controller's body to a caller who asked for another's — the mixed-segment
/// form was compared as a literal, never matched, and the address was quietly
/// declared unversioned. Loose in the direction the router can correct; never in
/// the direction it cannot see.
///
/// Allocation-free — it runs on every request.
fn route_matches(path: &str, pattern: &str) -> bool {
    let mut segments = path.split('/');
    let mut expected = pattern.split('/');
    loop {
        let (segment, pattern) = (segments.next(), expected.next());
        match (segment, pattern) {
            (None, None) => return true,
            // A catch-all takes the rest of the path, and `/cat/` is an address
            // it answers at — so it matches an empty tail too.
            (_, Some(pat)) if pat.starts_with('*') => return true,
            (Some(segment), Some(pat)) => {
                if !segment_matches(segment, pat) {
                    return false;
                }
            }
            // The pattern ran out: a trailing `/` on the request names the same
            // address, anything else is a longer path.
            (Some(segment), None) => {
                if !segment.is_empty() {
                    return false;
                }
            }
            (None, Some(_)) => return false,
        }
    }
}

/// One path segment against one pattern segment. A `:name` or `<regex>` matches
/// any non-empty text; a literal before it must still match, so `/@:handle`
/// accepts `@bob` and refuses `bob`.
fn segment_matches(segment: &str, pattern: &str) -> bool {
    match pattern.find([':', '<']) {
        None => segment == pattern,
        Some(0) => !segment.is_empty(),
        Some(literal) => segment.len() > literal && segment.starts_with(&pattern[..literal]),
    }
}

/// What a request said about the version it wants — and *that* it said
/// something, which is the distinction the answer turns on.
///
/// [`Requested::Absent`] and [`Requested::Malformed`] were one value once, and
/// collapsing them is a fail-open: a caller whose header cannot be decoded was
/// read as having asked for nothing, and served the default version, or the
/// unversioned neighbour at the same address, at `200`.
enum Requested<'a> {
    /// Nothing was stated, so a deployment default may apply.
    Absent,
    /// This was stated. Still to be validated as a version token.
    Stated(&'a str),
    /// Something was stated that is not text at all.
    Malformed,
}

/// The `version=` parameter of any media range in `Accept`. The first one wins:
/// a client listing two versions is asking two questions, and answering the
/// first is the only reading that does not invent a preference order the
/// header never stated.
///
/// An `Accept` that is not decodable as text is [`Requested::Malformed`] for the
/// same reason the header strategy's is: the caller stated something, and the
/// framework could not read it.
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
        // No `version=` parameter anywhere is a caller who asked for nothing —
        // an ordinary `Accept: application/json` — not a malformed statement.
        .map_or(Requested::Absent, Requested::Stated)
}

/// The one refusal both strategies raise, so a caller cannot tell which half of
/// the check refused them.
fn malformed_version() -> Error {
    Error::from_string("malformed API version", StatusCode::BAD_REQUEST)
}

/// A version token is spliced into a URL path, so it is validated before it
/// gets anywhere near one: bare alphanumerics, `.` and `-`, bounded length.
/// Everything else — a `/`, a `..`, an encoded byte — is a caller trying to
/// reach a path the API never mounted.
///
/// The rule is worded in `nest_rs_codegen::versioning`, which is where
/// `#[controller(version = …)]` reads it; this is a **copy**, because that crate
/// pulls `syn` and no app's dependency graph may. The copy is not left to drift:
/// `the_wire_grammar_matches_the_declared_grammar` in this module's tests holds
/// a dev-dependency on `nest-rs-codegen` and compares the two over a table.
fn is_valid_version(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= MAX_VERSION_LEN
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Rewrites a request's path from the version it asks for, then routes it.
/// Sits *inside* the global prefix, so the path it sees is the one controllers
/// mount at. Plumbing: `HttpTransport::api_versioning` is the seam.
pub(crate) struct VersionedEndpoint<E> {
    inner: E,
    selector: VersionSelector,
}

impl<E> VersionedEndpoint<E> {
    /// Wrap `inner` with `selector`.
    pub(crate) fn new(inner: E, selector: VersionSelector) -> Self {
        Self { inner, selector }
    }

    /// The path this request should be routed at, or `None` to route it as
    /// sent. `Err` is the refusal — a `404` or a `400` — decided here so the
    /// caller does nothing but apply the answer.
    fn resolve(&self, req: &Request) -> Result<Option<String>> {
        let path = req.uri().path();
        // A self-mounted endpoint owns its path outright and has no version to
        // be rewritten to. Absolutely neutral, and therefore genuinely first:
        // this test sat *below* the URI-form refusal, which meant a gateway
        // self-mounted at `/v1/ws` — `#[gateway(version = "1")]` goes through
        // the same `version_path` a controller does — was refused at its own
        // mount by the check meant for callers spelling a version by hand.
        if self.selector.is_self_mount(path) {
            return Ok(None);
        }

        // The mounted versioned routes *are* the URI form, so asking whether
        // the request already addresses one answers it exactly — for `/v1` and
        // for `#[controller(version = "2024-08-11")]` alike. A digits-only
        // heuristic missed the second and left it reachable two ways.
        if self.selector.is_versioned(path) {
            tracing::debug!(
                target: "nest_rs::http",
                path = path,
                strategy = self.selector.strategy.as_str(),
                "refused a URI-versioned path under a non-URI versioning strategy",
            );
            return Err(Error::from_status(StatusCode::NOT_FOUND));
        }

        let (version, stated) = match self.selector.requested(req) {
            Requested::Stated(raw) if is_valid_version(raw) => (Some(raw), true),
            Requested::Stated(raw) => {
                tracing::warn!(
                    target: "nest_rs::http",
                    strategy = self.selector.strategy.as_str(),
                    length = raw.len(),
                    "rejected a malformed API version",
                );
                return Err(malformed_version());
            }
            // Stated, and not decodable as text at all. Treating this as
            // "nothing was stated" served the default version — or an
            // unversioned neighbour's body — at `200`, so one non-ASCII byte
            // decided which controller answered.
            Requested::Malformed => {
                tracing::warn!(
                    target: "nest_rs::http",
                    strategy = self.selector.strategy.as_str(),
                    reason = "not valid text",
                    "rejected a malformed API version",
                );
                return Err(malformed_version());
            }
            Requested::Absent => (self.selector.default_version(), false),
        };

        // A default never rewrites an address an unversioned controller already
        // answers: the caller asked for nothing, so nothing moves under them.
        // A *stated* version does — an explicit request is the strongest signal
        // there is, and yielding to an unversioned neighbour would leave
        // `#[controller(version = …)]` unreachable at that address.
        if !stated && self.selector.is_unversioned(path) {
            return Ok(None);
        }

        let Some(version) = version else {
            return Ok(None);
        };
        let candidate = version_path(Some(version), path);
        if self.selector.is_versioned(&candidate) {
            return Ok(Some(candidate));
        }
        if stated && self.selector.has_any_version(path) {
            // The caller named a version this address does not serve, while
            // another one does. Falling through would answer with that other
            // version's body — the silent fallback this strategy exists to
            // avoid.
            tracing::debug!(
                target: "nest_rs::http",
                path = path,
                "no route serves the requested API version",
            );
            return Err(Error::from_status(StatusCode::NOT_FOUND));
        }
        // Nothing versioned answers here at all: served as written, and the
        // router has the last word either way.
        Ok(None)
    }
}

impl<E> Endpoint for VersionedEndpoint<E>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Response> {
        // Resolved behind an immutable borrow so the common outcomes — neutral
        // address, no version stated — allocate **nothing**. The path used to be
        // cloned on entry, before it was known whether anything would use it,
        // and this endpoint sits in front of every request under a non-URI
        // strategy. The one `String` left is the rewritten path itself.
        if let Some(path) = self.resolve(&req)? {
            rewrite_path(&mut req, &path)?;
        }
        self.inner.call(req).await.map(IntoResponse::into_response)
    }
}

/// Swap the request's path, keeping its query. `original_uri` is untouched, so
/// the access log and every `#[meta]` reader still see what the client sent.
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
            // Unreachable in practice: `path` is `version_path` over a path the
            // router already parsed, and the version passed validation. Refuse
            // rather than route an unrewritten request to the wrong version.
            Error::from_status(StatusCode::BAD_REQUEST)
        },
    )?);
    *req.uri_mut() = Uri::from_parts(parts).map_err(|_| {
        Error::from_string("could not resolve the API version", StatusCode::BAD_REQUEST)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {

    use poem::test::TestClient;
    use poem::{Route, get, handler};

    use super::*;

    #[handler]
    fn v1() -> &'static str {
        "one"
    }

    #[handler]
    fn v2() -> &'static str {
        "two"
    }

    #[handler]
    fn unversioned() -> &'static str {
        "none"
    }

    fn app(strategy: ApiVersioning, default_version: Option<&str>) -> VersionedEndpoint<Route> {
        let route = Route::new()
            .at("/v1/users", get(v1))
            .at("/v2/users", get(v2))
            .at("/users", get(unversioned));
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
    fn the_matcher_reads_every_segment_form_the_router_parses() {
        // Each line here was a defect an audit reproduced, or the shape that
        // defect hid behind.
        assert!(route_matches("/posts/abc", "/posts/:id"));
        // A literal and a parameter in one segment — a handle, a slug. Compared
        // as a literal this never matched, and the address was then declared
        // unversioned while a versioned route served it.
        assert!(route_matches("/mix/@bob", "/mix/@:handle"));
        assert!(!route_matches("/mix/bob", "/mix/@:handle"));
        assert!(route_matches("/r/report-7", "/r/report-:id"));
        // A regex segment is *one* segment, not a tail — reading it as a
        // catch-all took an unversioned neighbour offline.
        assert!(route_matches("/probe/7", r"/probe/<\d+>"));
        assert!(!route_matches("/probe/archive/7", r"/probe/<\d+>"));
        // A catch-all takes the rest, and `/cat/` is one of its addresses.
        assert!(route_matches("/cat/a/b", "/cat/*rest"));
        assert!(route_matches("/cat/", "/cat/*rest"));
        // A trailing slash names the same address; anything longer does not.
        assert!(route_matches("/users/", "/users"));
        assert!(!route_matches("/users/1", "/users"));
        assert!(!route_matches("/postsy", "/posts"));
    }

    #[test]
    fn a_selector_with_nothing_versioned_reports_itself_inert() {
        // The transport reads this to skip the wrap. An endpoint that can only
        // ever pass the request through still costs a routing layer on every
        // request — measured at +57% — so "can this change any outcome?" has to
        // be answerable without running it.
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
        // The two ends prefix matching got wrong, and the parameter syntax the
        // router actually mounts with.
        assert!(route_matches("/ping", "/ping"));
        assert!(route_matches("/posts/abc", "/posts/:id"));
        assert!(route_matches("/files/a/b/c", "/files/*rest"));
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
            !route_matches("/posts", "/posts/:id"),
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
        let client = TestClient::new(app(ApiVersioning::Header, None));
        let resp = client
            .get("/users")
            .header(DEFAULT_VERSION_HEADER, "2")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("two").await;
    }

    #[tokio::test]
    async fn a_media_type_parameter_selects_the_version() {
        let client = TestClient::new(app(ApiVersioning::MediaType, None));
        let resp = client
            .get("/users")
            .header(header::ACCEPT, "application/json; version=1")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("one").await;
    }

    #[tokio::test]
    async fn a_quoted_media_type_parameter_is_read_the_same() {
        let client = TestClient::new(app(ApiVersioning::MediaType, None));
        let resp = client
            .get("/users")
            .header(header::ACCEPT, "text/html, application/json;version=\"2\"")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("two").await;
    }

    #[tokio::test]
    async fn a_default_never_moves_an_address_that_already_answers_without_one() {
        // The fixture mounts `/users` three ways: v1, v2, and unversioned. A
        // caller who states nothing gets the route the app mounted for exactly
        // that request — the unversioned one — even under a default version.
        //
        // A default is the weakest signal in the system: the caller asked for
        // nothing, so nothing moves under them. Letting it win here is what let
        // a versioned catch-all controller swallow `/health/live` and every
        // other unversioned route beside it. A developer who wants `/users` to
        // mean v1 does not also mount an unversioned `/users`.
        for default in [Some("1"), None] {
            let client = TestClient::new(app(ApiVersioning::Header, default));
            let resp = client.get("/users").send().await;
            resp.assert_status_is_ok();
            resp.assert_text("none").await;
        }

        // A *stated* version is the strongest, and still wins at that address.
        let client = TestClient::new(app(ApiVersioning::Header, Some("1")));
        let resp = client
            .get("/users")
            .header(DEFAULT_VERSION_HEADER, "2")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("two").await;
    }

    #[tokio::test]
    async fn the_query_string_survives_the_rewrite() {
        #[handler]
        fn echo(req: &Request) -> String {
            req.uri().query().unwrap_or("").to_owned()
        }

        let ep = VersionedEndpoint::new(
            Route::new().at("/v2/users", get(echo)),
            VersionSelector::new(
                ApiVersioning::Header,
                HeaderName::from_static(DEFAULT_VERSION_HEADER),
                None,
            )
            .with_routes(vec!["/v2/users".into()], vec![], vec![], vec!["2".into()]),
        );
        let resp = TestClient::new(ep)
            .get("/users?first=10&after=abc")
            .header(DEFAULT_VERSION_HEADER, "2")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("first=10&after=abc").await;
    }

    #[tokio::test]
    async fn a_version_that_could_reach_another_path_is_refused() {
        // The token is spliced into a path, so this is path traversal, not a
        // typo: refuse it rather than let the router decide what it means.
        let client = TestClient::new(app(ApiVersioning::Header, None));
        for probe in [
            "../admin",
            "1/../../etc",
            "1%2f2",
            "",
            "a".repeat(64).as_str(),
        ] {
            let resp = client
                .get("/users")
                .header(DEFAULT_VERSION_HEADER, probe)
                .send()
                .await;
            assert_eq!(
                resp.0.status(),
                StatusCode::BAD_REQUEST,
                "version {probe:?} must be refused",
            );
        }
    }

    #[tokio::test]
    async fn a_uri_versioned_path_is_not_a_second_address_under_another_strategy() {
        let client = TestClient::new(app(ApiVersioning::Header, None));
        let resp = client.get("/v2/users").send().await;
        resp.assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_uri_form_is_refused_whatever_the_version_is_spelled_like() {
        // A version is an opaque string, so a `v` + digits test would leave
        // `#[controller(version = "2024-08-11")]` reachable at both addresses.
        let route = Route::new().at("/v2024-08-11/users", get(v2));
        let ep = VersionedEndpoint::new(
            route,
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
        let client = TestClient::new(ep);
        client
            .get("/v2024-08-11/users")
            .send()
            .await
            .assert_status(StatusCode::NOT_FOUND);

        let resp = client
            .get("/users")
            .header(DEFAULT_VERSION_HEADER, "2024-08-11")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("two").await;
    }

    #[tokio::test]
    async fn a_path_that_merely_starts_with_v_is_left_alone() {
        // `/vendors` is not a version segment, and nothing versioned is mounted
        // under it, so the selector must not touch it.
        let ep = VersionedEndpoint::new(
            Route::new().at("/vendors", get(unversioned)),
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
        let resp = TestClient::new(ep).get("/vendors").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("none").await;
    }

    /// The transport keeps its route tree typed as a `Route` so the no-layer
    /// fast path stays monomorphized; folding the rewrite back in through a
    /// root `nest_no_strip` is what preserves that. Pin the behaviour poem
    /// gives us there.
    #[tokio::test]
    async fn a_root_nest_no_strip_routes_the_full_path() {
        let wrapped = VersionedEndpoint::new(
            Route::new().at("/v2/users", get(v2)),
            VersionSelector::new(
                ApiVersioning::Header,
                HeaderName::from_static(DEFAULT_VERSION_HEADER),
                None,
            )
            .with_routes(vec!["/v2/users".into()], vec![], vec![], vec!["2".into()]),
        );
        let resp = TestClient::new(Route::new().nest_no_strip("/", wrapped))
            .get("/users")
            .header(DEFAULT_VERSION_HEADER, "2")
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("two").await;
    }

    /// The wire grammar and the declared grammar are one rule, and this is what
    /// keeps the copy honest.
    ///
    /// `nest_rs_codegen` words it; `#[controller(version = …)]` reads it there.
    /// This crate cannot — `nest-rs-codegen` pulls `syn`, and the umbrella rule
    /// forbids that reaching an app's dependency graph — so it carries the
    /// predicate itself and pins it here, through a dev-dependency.
    ///
    /// The bound is the half that was missing: `MAX_VERSION_LEN` lived only on
    /// this side, so a 40-character declared version compiled, mounted, logged
    /// and documented, and was then refused with `400` the moment a caller
    /// named it.
    #[test]
    fn the_wire_grammar_matches_the_declared_grammar() {
        assert_eq!(
            MAX_VERSION_LEN,
            nest_rs_codegen::versioning::MAX_VERSION_LEN
        );

        // Exhaustive over the byte space rather than over a table of examples:
        // a table passes for every character it does not list, which is exactly
        // the drift this test exists to catch. One-character strings settle the
        // character set; the sweep around the bound settles the length.
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
        // And the shapes a caller actually sends, including the ones a version
        // token must never be able to become.
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
