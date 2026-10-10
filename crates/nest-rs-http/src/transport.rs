use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use nest_rs_core::{Container, Discovery, Transport};
use poem::endpoint::BoxEndpoint;
use poem::http::header::{HeaderName, HeaderValue, SERVER};
use poem::listener::{Listener, TcpListener};
use poem::middleware::{Compression, Cors};
use poem::{EndpointExt, IntoEndpoint, Response, Route, Server};
use tokio_util::sync::CancellationToken;

use crate::boot_check::{GlobalGuardsActive, HttpBootCheck};
use crate::controller::HttpControllerMeta;
use crate::detached::DetachedWork;
use crate::drain::Drain;
use crate::endpoint::{EdgePosture, HttpEndpointMeta, SelfMountGuardWrap};
use crate::fallback::{Claims, Fallback, HttpFallbackMeta, WithFallback, literal_prefix};
use crate::interceptor::HttpEndpointWrap;
use crate::tls::HttpTls;
use crate::versioning::VersionedEndpoint;

type MountFn = Box<dyn Fn(&Container, Route) -> Route + Send + Sync>;
/// Imperative mount paired with its path, so the fail-secure check can name it.
type NamedMount = (String, MountFn);

/// Join a controller prefix with a route path the way poem's nesting does:
/// `("/health", "/live") -> "/health/live"`.
pub fn join_path(prefix: &str, rest: &str) -> String {
    let p = prefix.trim_end_matches('/');
    let r = rest.trim_start_matches('/');
    match (p.is_empty(), r.is_empty()) {
        (true, true) => "/".to_string(),
        (false, true) => p.to_string(),
        (true, false) => format!("/{r}"),
        (false, false) => format!("{p}/{r}"),
    }
}

/// Apply URI API versioning: `Some("1"), "/users"` → `"/v1/users"`.
pub fn version_path(version: Option<&str>, path: &str) -> String {
    match version {
        Some(v) => join_path(&format!("/v{v}"), path),
        None => path.to_string(),
    }
}

/// Does `declared` contain every version in `named`?
///
/// `#[routes]` asserts it in `const` per `#[version("…")]` route: the mount loops
/// over the controller's versions, so an undeclared one would compile and answer nothing.
pub const fn versions_declare(declared: &[&str], named: &[&str]) -> bool {
    let mut i = 0;
    while i < named.len() {
        let mut found = false;
        let mut j = 0;
        while j < declared.len() {
            if const_str_eq(declared[j], named[i]) {
                found = true;
                break;
            }
            j += 1;
        }
        if !found {
            return false;
        }
        i += 1;
    }
    true
}

/// `==` on `&str` is not `const`; this is.
const fn const_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// HTTP [`Transport`] backed by poem: every discovered controller and
/// self-mounted endpoint, then each [`mount`](Self::mount), under the
/// discovered transport-level wraps.
pub struct HttpTransport {
    bind: String,
    mounts: Vec<NamedMount>,
    cors: Option<Cors>,
    tls: Option<HttpTls>,
    server_header: Option<&'static str>,
    global_prefix: Option<String>,
    max_body_bytes: Option<usize>,
    request_timeout: Option<Duration>,
    shutdown_timeout: Duration,
    fail_secure_strict: bool,
    security_headers: crate::HttpSecurityHeaders,
    compression: bool,
    version_selector: Option<crate::VersionSelector>,
    /// What each self-mount runs off its connections, by mount path.
    detached: Vec<(String, DetachedWork)>,
    /// Shared with the edge, whose response bodies read it.
    drain: Arc<Drain>,
    endpoint: Option<BoxEndpoint<'static, Response>>,
}

fn normalize_global_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("/{trimmed}"))
}

/// The canonical form of a mount path: `"/x"`, with the root as `"/"`.
///
/// The collision checks compare mount paths as strings, while `Route::nest` makes
/// `"/x"` and `"/x/"` one key: left raw, the pair slips past and poem panics.
pub fn normalize_mount_path(raw: &str) -> String {
    match normalize_global_prefix(raw) {
        Some(path) => path,
        None => "/".to_owned(),
    }
}

/// `raw` as [`normalize_mount_path`] writes it, when every segment is a
/// literal the router matches as written: none empty, `.` or `..`, none
/// holding what poem reads as pattern syntax (`:name`, `<regex>`, `*rest`), nor
/// `%`, `?`, `#`, `\`, whitespace or a control character. `None` otherwise.
pub fn literal_mount_path(raw: &str) -> Option<String> {
    let path = normalize_mount_path(raw);
    let literal = path.split('/').skip(1).all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !is_pattern_segment(segment)
            && !segment.contains(['%', '?', '#', '\\'])
            && !segment.chars().any(|c| c.is_whitespace() || c.is_control())
    });
    (path == "/" || literal).then_some(path)
}

/// Where a capture opens in a route pattern's segment — `:name` or `<regex>`,
/// after any literal it is glued to (`@:handle`).
pub(crate) fn parameter_start(segment: &str) -> Option<usize> {
    segment.find([':', '<'])
}

/// Whether a route pattern's segment is anything but a literal: a capture, or
/// a `*rest` catch-all.
pub(crate) fn is_pattern_segment(segment: &str) -> bool {
    segment.starts_with('*') || parameter_start(segment).is_some()
}

/// Claim `path` for `owner`, or fail boot naming both claimants — before poem
/// panics in route assembly (`duplicate path: <prefix>/*--poem-rest`).
fn claim_exclusive_path(
    owners: &mut HashMap<String, String>,
    kind: &str,
    path: String,
    owner: String,
    remedy: &str,
) -> anyhow::Result<()> {
    if let Some(first) = owners.insert(path.clone(), owner.clone()) {
        anyhow::bail!(
            "duplicate {kind} {path:?}: {first} and {owner} both mount there — a {kind} is its \
             exclusive namespace; {remedy}",
        );
    }
    Ok(())
}

/// Mount the router's fallback, or fail the boot when its path lies under a
/// prefix someone owns, where the router would answer everything first.
fn mount_fallback(
    container: &Container,
    meta: &HttpFallbackMeta,
    claims: Claims,
    get_routes: &[(String, &'static str)],
) -> anyhow::Result<Fallback> {
    if let Some((prefix, owner)) = claims.owner_of(meta.path()) {
        anyhow::bail!(
            "the router fallback {} answers at {:?}, under {prefix:?}, which {owner} owns: \
             the route table claims everything there, so the fallback would answer nothing — \
             give it a path outside {prefix:?}",
            meta.owner(),
            meta.path(),
        );
    }
    if let Some((_, controller)) = get_routes.iter().find(|(path, _)| path == meta.path()) {
        tracing::warn!(
            target: crate::target::ROUTES,
            path = meta.path(),
            controller,
            fallback = meta.owner(),
            "a route answers GET at the router fallback's own path, which the fallback never \
             answers",
        );
    }
    tracing::info!(
        target: crate::target::ROUTES,
        kind = "fallback",
        path = meta.path(),
        "mounted endpoint",
    );
    Ok(Fallback::new(
        meta.path().to_owned(),
        claims,
        meta.mount(container).boxed(),
    ))
}

/// What to do about two controllers claiming one mount prefix.
fn prefix_remedy(version: Option<&str>) -> String {
    match version {
        Some(version) => format!(
            "both declare version {version:?} at that path — drop it from one of the two \
             `#[controller(version = …)]` lists",
        ),
        None => "give each one a distinct path".to_owned(),
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport {
    /// A transport with framework defaults: bind `0.0.0.0:3000`, no TLS or CORS,
    /// fail-secure strict.
    pub fn new() -> Self {
        Self {
            bind: "0.0.0.0:3000".into(),
            mounts: Vec::new(),
            cors: None,
            tls: None,
            server_header: None,
            global_prefix: None,
            max_body_bytes: None,
            request_timeout: None,
            shutdown_timeout: crate::config::DEFAULT_SHUTDOWN_TIMEOUT,
            fail_secure_strict: true,
            security_headers: crate::HttpSecurityHeaders::default(),
            compression: false,
            // `None` is the URI strategy: the version is already in the mount path.
            version_selector: None,
            detached: Vec::new(),
            drain: Arc::default(),
            endpoint: None,
        }
    }

    /// Build the transport an [`HttpConfig`](crate::HttpConfig) describes.
    pub fn from_config(cfg: &crate::HttpConfig) -> anyhow::Result<Self> {
        let mut http = Self::new().bind(format!("{}:{}", cfg.host, cfg.port));
        if let Some(tls) = cfg.tls.clone() {
            http = http.tls(tls);
        }
        if let Some(cors) = cfg.cors.clone() {
            http = http.cors(cors.into_middleware()?);
        }
        if cfg.server_header {
            http = http.server_header(concat!("nestrs/", env!("CARGO_PKG_VERSION")));
        }
        if let Some(prefix) = cfg.global_prefix.clone() {
            http = http.global_prefix(prefix);
        }
        if let Some(selector) = cfg.version_selector() {
            http = http.api_versioning(selector);
        }
        http = http.max_body_bytes(cfg.max_body_bytes.unwrap_or(crate::RawBody::DEFAULT_LIMIT));
        if let Some(timeout) = cfg.request_timeout {
            http = http.request_timeout(timeout);
        }
        http = http.shutdown_timeout(cfg.shutdown_timeout);
        http = http.fail_secure_strict(cfg.fail_secure_strict);
        http = http.security_headers(cfg.security_headers.clone());
        http = http.compression(cfg.compression);
        Ok(http)
    }

    /// Resolve each request's API version through `selector` instead of from
    /// its path.
    pub fn api_versioning(mut self, selector: crate::VersionSelector) -> Self {
        self.version_selector = Some(selector);
        self
    }

    /// Pin the default security-header policy; the default is nosniff,
    /// `X-Frame-Options: DENY` and HSTS under TLS.
    pub fn security_headers(mut self, cfg: crate::HttpSecurityHeaders) -> Self {
        self.security_headers = cfg;
        self
    }

    /// `true` (the default) makes `configure` **fail** when global guards are
    /// registered and an imperative [`mount`](Self::mount) endpoint would
    /// bypass the guard pool; `false` downgrades the violation to a `warn`.
    pub fn fail_secure_strict(mut self, strict: bool) -> Self {
        self.fail_secure_strict = strict;
        self
    }

    /// Mount every controller under a shared prefix (e.g. `/api`). Empty or
    /// `"/"` is no prefix; a missing leading `/` is added, a trailing one stripped.
    pub fn global_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.global_prefix = normalize_global_prefix(&prefix.into());
        self
    }

    /// Emit `Server: <value>` on every response; off by default.
    pub fn server_header(mut self, value: &'static str) -> Self {
        self.server_header = Some(value);
        self
    }

    /// Set the listen address (`host:port`).
    pub fn bind(mut self, addr: impl Into<String>) -> Self {
        self.bind = addr.into();
        self
    }

    /// Cap each request's raw body to `limit` bytes, which [`RawBody`](crate::RawBody)
    /// reads through [`current_body_limit`](crate::current_body_limit).
    pub fn max_body_bytes(mut self, limit: usize) -> Self {
        self.max_body_bytes = Some(limit);
        self
    }

    /// Abort any request that runs longer than `timeout`, answering `503 Service
    /// Unavailable` with a `Retry-After`. Without this call no timeout is enforced.
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// How long [`serve`](Transport::serve) lets open connections finish once
    /// shutdown is asked for, 20 seconds by default. A body with no end of its
    /// own ([`OpenEndedBody`](crate::OpenEndedBody)) ends at the signal; a
    /// connection still open at the window's close is cut, and one `warn` counts them.
    pub fn shutdown_timeout(mut self, window: Duration) -> Self {
        self.shutdown_timeout = window;
        self
    }

    /// Enable CORS with a configured poem [`Cors`] middleware, outermost so a
    /// preflight is answered before any guard or interceptor runs.
    pub fn cors(mut self, cors: Cors) -> Self {
        self.cors = Some(cors);
        self
    }

    /// Negotiate response compression from each request's `Accept-Encoding`
    /// (poem's [`Compression`]: gzip, deflate, brotli, zstd); off by default.
    pub fn compression(mut self, on: bool) -> Self {
        self.compression = on;
        self
    }

    /// Serve HTTPS directly from [`HttpTls`] (poem's `rustls` listener)
    /// instead of plain HTTP. Without this call the transport stays plaintext.
    pub fn tls(mut self, tls: HttpTls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Mount an extra endpoint at `path`. The builder closure runs at
    /// [`Transport::configure`] time with the live container, so it can
    /// resolve services to construct framework-specific endpoints.
    pub fn mount<F, E>(mut self, path: impl Into<String>, build: F) -> Self
    where
        F: Fn(&Container) -> E + Send + Sync + 'static,
        E: IntoEndpoint,
        E::Endpoint: 'static,
        <E::Endpoint as poem::Endpoint>::Output: poem::IntoResponse,
    {
        let path = path.into();
        let mount_path = path.clone();
        self.mounts.push((
            path,
            Box::new(move |container, route| {
                let endpoint =
                    crate::matched(build(container).into_endpoint().map_to_response()).boxed();
                route.nest(mount_path.clone(), endpoint)
            }),
        ));
        self
    }

    /// Take the assembled endpoint for in-process testing (drive with poem's
    /// `TestClient`). Returns `None` before `configure` has run, and leaves
    /// the transport without an endpoint (so it must not also be `serve`d).
    pub fn take_endpoint(&mut self) -> Option<BoxEndpoint<'static, Response>> {
        self.endpoint.take()
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        let discovery = Discovery::new(container);
        // Before anything mounts: an unresolvable global layer spec would
        // otherwise be dropped silently.
        for d in discovery.meta::<HttpBootCheck>() {
            d.meta.run(container).map_err(|msg| anyhow::anyhow!(msg))?;
        }
        let mut route = Route::new();

        let global_guards = container.get::<GlobalGuardsActive>().is_some();
        let mut unguarded: Vec<String> = Vec::new();
        let mut prefix_owner: HashMap<String, String> = HashMap::new();
        // Routes mount flat (`<prefix>/<path>`), so distinct prefixes can still
        // collide on a full path; several verbs of one controller share an entry.
        let mut route_owner: HashMap<String, String> = HashMap::new();

        // Full routes, not prefixes, for the non-URI rewrite: a `/` prefix matches
        // nothing segment-wise, and `/posts` would claim another controller's `/posts/drafts`.
        let mut versioned_routes: Vec<String> = Vec::new();
        // Kept apart because they yield differently: a self-mount is neutral against
        // anything, an unversioned controller route only against a *default* version.
        let mut self_mounts: Vec<String> = Vec::new();
        let mut unversioned_routes: Vec<String> = Vec::new();
        // What the router's fallback never answers. A controller's prefix is its
        // namespace although its routes mount flat; one at `/` owns its routes alone.
        let mut claims = Claims::default();
        let mut get_routes: Vec<(String, &'static str)> = Vec::new();
        for d in discovery.meta::<HttpControllerMeta>() {
            let at_root = literal_prefix(d.meta.path) == "/";
            if !at_root {
                // The address a non-URI version selector is called at.
                claims.prefix(d.meta.path, d.meta.controller);
            }
            for version in d.meta.mounted_versions() {
                let prefix = d.meta.effective_prefix(version);
                if !at_root {
                    claims.prefix(&prefix, d.meta.controller);
                }
                claim_exclusive_path(
                    &mut prefix_owner,
                    "controller prefix",
                    prefix.clone(),
                    d.meta.controller.to_owned(),
                    &prefix_remedy(version),
                )?;
                for r in &d.meta.routes {
                    if !HttpControllerMeta::serves(r, version) {
                        continue;
                    }
                    let path = join_path(&prefix, r.path);
                    // Under a non-URI selector a versioned route is called unversioned.
                    let addresses = match version {
                        Some(_) => vec![path.clone(), join_path(d.meta.path, r.path)],
                        None => vec![path.clone()],
                    };
                    for address in addresses {
                        if r.verb == crate::HttpVerb::Get {
                            get_routes.push((address.clone(), d.meta.controller));
                        }
                        if at_root {
                            claims.route(address);
                        }
                    }
                    match version.is_some() {
                        true => versioned_routes.push(path.clone()),
                        false => unversioned_routes.push(path.clone()),
                    }
                    if let Some(first) =
                        route_owner.insert(path.clone(), d.meta.controller.to_owned())
                        && first != d.meta.controller
                    {
                        anyhow::bail!(
                            "duplicate route path {path:?}: {first} and {} both mount there — \
                             give each controller route a distinct full path",
                            d.meta.controller,
                        );
                    }
                    // Under a non-URI strategy `/v{n}` is where the route mounts, not
                    // what a client calls, so the version is logged as its own field.
                    let (logged, version) = match &self.version_selector {
                        Some(_) => (join_path(d.meta.path, r.path), version),
                        None => (path.clone(), None),
                    };
                    tracing::info!(
                        target: crate::target::ROUTES,
                        controller = d.meta.controller,
                        method = r.verb.as_str(),
                        path = logged.as_str(),
                        version = version,
                        handler = r.handler,
                        "mounted route",
                    );
                    if r.access_is_implicit(global_guards) {
                        unguarded.push(format!("{} {} ({})", r.verb.as_str(), path, r.handler));
                    }
                }
            }
            route = d.meta.mount(container, route);
        }

        // Unchecked, an undeclared default would silently send every caller that
        // states no version to the unversioned route or a 404.
        if let Some(selector) = &self.version_selector
            && selector.rewrites()
            && let Some(default) = selector.default_version()
        {
            let declared = crate::declared_versions(container);
            if !declared.iter().any(|v| v == default) {
                let var = nest_rs_config::var_name("http", "DEFAULT_VERSION");
                anyhow::bail!(match declared.is_empty() {
                    true => format!(
                        "{var} names API version {default:?}, and no controller declares a \
                         version at all — declare it with #[controller(version = {default:?})] \
                         or unset {var}",
                    ),
                    false => format!(
                        "{var} names API version {default:?}, which no controller declares — \
                         the versions mounted are {}; name one of those or unset {var}",
                        declared.join(", "),
                    ),
                });
            }
        }

        if !unguarded.is_empty() {
            tracing::warn!(
                target: nest_rs_core::target::LAYERS,
                count = unguarded.len(),
                routes = unguarded.join(", ").as_str(),
                hint = "bind a guard or mark them #[public]",
                "unguarded routes detected",
            );
        }
        // Absent without a global guard pool. A `Guarded` self-mount has no route
        // shaper, so the transport runs the pool at its edge.
        let self_mount_guard = discovery
            .meta::<SelfMountGuardWrap>()
            .into_iter()
            .next()
            .map(|d| d.meta);
        // A warning, not a fail-secure stop: a gateway may bind its own
        // `#[use_guards]` inside its opaque mount closure.
        let mut unguarded_edges: Vec<String> = Vec::new();
        let mut endpoint_owner: HashMap<String, String> = HashMap::new();
        for d in discovery.meta::<HttpEndpointMeta>() {
            // A self-mount nests its whole subtree, so a controller on that path
            // is the same poem panic.
            if let Some(first) = prefix_owner
                .get(d.meta.path())
                .or_else(|| route_owner.get(d.meta.path()))
            {
                anyhow::bail!(
                    "duplicate mount path {:?}: controller {first} and {} endpoint {} both mount \
                     there — a mount path is its owner's exclusive namespace; give each one a \
                     distinct path",
                    d.meta.path(),
                    d.meta.label(),
                    d.meta.owner(),
                );
            }
            // Recorded unversioned so a versioned catch-all cannot swallow them;
            // exactly the declared paths — a surface owning a subtree says so via `also_mounts`.
            for path in d.meta.paths() {
                self_mounts.push(path.to_owned());
                claims.prefix(
                    path,
                    format!("{} endpoint {}", d.meta.label(), d.meta.owner()),
                );
                claim_exclusive_path(
                    &mut endpoint_owner,
                    "self-mounted endpoint path",
                    path.to_owned(),
                    format!("{} endpoint {}", d.meta.label(), d.meta.owner()),
                    "give each one a distinct path",
                )?;
            }
            tracing::info!(
                target: crate::target::ROUTES,
                kind = d.meta.label(),
                path = d.meta.path(),
                "mounted endpoint",
            );
            if let Some(work) = d.meta.detached() {
                self.detached.push((d.meta.path().to_owned(), work.clone()));
            }
            if d.meta.edge_access_is_implicit(global_guards) {
                unguarded_edges.push(format!("{} ({})", d.meta.path(), d.meta.label()));
            }
            match (d.meta.posture(), &self_mount_guard) {
                (EdgePosture::Guarded, Some(wrap)) => {
                    // `nest_no_strip`: the isolated sub-route matches its own full path.
                    let isolated: BoxEndpoint<'static, Response> =
                        d.meta.mount(container, Route::new()).boxed();
                    let wrapped = wrap.apply(container, isolated);
                    route = route.nest_no_strip(d.meta.path(), wrapped);
                }
                _ => {
                    // `Exempt`: gated in band (GraphQL, MCP) or public by design (OpenAPI).
                    route = d.meta.mount(container, route);
                }
            }
        }
        if !unguarded_edges.is_empty() {
            tracing::warn!(
                target: nest_rs_core::target::LAYERS,
                count = unguarded_edges.len(),
                endpoints = unguarded_edges.join(", ").as_str(),
                hint = "register a global guard pool or gate the gateway with #[use_guards]",
                "unguarded self-mount edges detected",
            );
        }
        // An imperative `mount` is opaque to the transport, so it bypasses the
        // global guard pool.
        if !self.mounts.is_empty() && container.get::<GlobalGuardsActive>().is_some() {
            let paths: Vec<&str> = self.mounts.iter().map(|(p, _)| p.as_str()).collect();
            if self.fail_secure_strict {
                anyhow::bail!(
                    "fail-secure: imperative mount(...) endpoints bypass the global guard pool: \
                     {} — route them through a #[controller], guard them explicitly, or opt out \
                     with HttpTransport::fail_secure_strict(false) / {}=false",
                    paths.join(", "),
                    nest_rs_config::var_name("http", "FAIL_SECURE_STRICT"),
                );
            }
            tracing::warn!(
                target: crate::target::HTTP,
                paths = paths.join(", ").as_str(),
                hint = "route through a #[controller] or guard explicitly",
                "imperative mounts bypass the global guard pool",
            );
        }
        for (path, mount) in self.mounts.drain(..) {
            claims.prefix(
                &path,
                format!("the endpoint HttpTransport::mount added at {path:?}"),
            );
            route = mount(container, route);
        }

        // Inside the global prefix, so it rewrites the path controllers mount at.
        // `rewrites()`, not `Some`: wrapping the URI strategy would refuse every `/v{n}/…`.
        if let Some(selector) = self.version_selector.take().filter(|s| s.rewrites()) {
            let selector = selector.with_routes(
                versioned_routes,
                self_mounts,
                unversioned_routes,
                crate::declared_versions(container),
            );
            // An inert selector changes nothing and costs a routing layer (+57% on the hot path).
            if !selector.is_inert() {
                route = Route::new().nest_no_strip("/", VersionedEndpoint::new(route, selector));
            }
        }

        if let Some(prefix) = self.global_prefix.take() {
            let owner = format!(
                "the global prefix ({})",
                nest_rs_config::var_name("http", "GLOBAL_PREFIX"),
            );
            claims.only(prefix.clone(), owner);
            // Every route answers under the prefix, never at a path of its own.
            get_routes.clear();
            route = Route::new().nest(prefix, route);
        }
        let fallback = match discovery.meta::<HttpFallbackMeta>().as_slice() {
            [] => None,
            [only] => Some(mount_fallback(container, &only.meta, claims, &get_routes)?),
            [first, second, ..] => anyhow::bail!(
                "two router fallbacks: {} and {} — the router has one fallback, which answers \
                 whatever nothing else claims; import one of them",
                first.meta.owner(),
                second.meta.owner(),
            ),
        };
        let route = WithFallback::new(route, fallback);

        // Ascending priority, whatever the registration order; the stable sort
        // keeps insertion order within a band.
        let mut metas: Vec<std::sync::Arc<HttpEndpointWrap>> = discovery
            .meta::<HttpEndpointWrap>()
            .into_iter()
            .map(|d| d.meta)
            .collect();
        metas.sort_by_key(|m| m.priority());
        let mut edge_headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
        for (name, value) in self.security_headers.headers(self.tls.is_some()) {
            // Values are boot-validated: a failure here is a framework bug.
            match HeaderValue::from_str(&value) {
                Ok(header_value) => edge_headers.push((name, header_value)),
                Err(_) => tracing::error!(
                    target: crate::target::HTTP,
                    header = name.as_str(),
                    "failed to construct a security header despite boot validation",
                ),
            }
        }
        if let Some(value) = self.server_header.take() {
            edge_headers.push((SERVER, HeaderValue::from_static(value)));
        }
        // The problem normalizer must be outermost: without CORS or compression
        // that is the edge itself, otherwise a wrap outside both.
        let fuse_normalize = !self.compression && self.cors.is_none();
        let timeout = self.request_timeout.take();
        let body_limit = self.max_body_bytes.take();

        let endpoint: BoxEndpoint<'static, Response> = if metas.is_empty() && fuse_normalize {
            crate::edge::EdgeEndpoint::new(
                route.map_to_response(),
                container.clone(),
                timeout,
                body_limit,
                edge_headers,
                true,
                // No compression outside, so nothing can rewrite the body.
                false,
                Arc::clone(&self.drain),
            )
            .boxed()
        } else {
            // The router answers 404/405 with an `Err`, which would short-circuit an
            // interceptor's `next.run(req).await?`; rendering at `ERROR_RESOLVE` lets them see it.
            let split = metas
                .partition_point(|m| m.priority() < crate::interceptor::priority::ERROR_RESOLVE);
            let (below, above) = metas.split_at(split);
            let mut endpoint: BoxEndpoint<'static, Response> = if below.is_empty() {
                crate::problem::ResolvedErrors(route).boxed()
            } else {
                let mut inner: BoxEndpoint<'static, Response> = route.map_to_response().boxed();
                for meta in below {
                    inner = meta.wrap(container, inner);
                }
                crate::problem::ResolvedErrors(inner).boxed()
            };
            for meta in above {
                endpoint = meta.wrap(container, endpoint);
            }
            let mut endpoint: BoxEndpoint<'static, Response> = crate::edge::EdgeEndpoint::new(
                endpoint,
                container.clone(),
                timeout,
                body_limit,
                edge_headers,
                fuse_normalize,
                // Compression replaces the request body but not its `Content-Length`.
                self.compression,
                Arc::clone(&self.drain),
            )
            .boxed();
            if self.compression {
                endpoint = endpoint.with(Compression::new()).map_to_response().boxed();
            }
            // Outside the edge, so a preflight carries no request scope.
            if let Some(cors) = self.cors.take() {
                endpoint = endpoint.with(cors).map_to_response().boxed();
            }
            if fuse_normalize {
                endpoint
            } else {
                endpoint
                    .around(|ep, req| async move {
                        Ok(match ep.call(req).await {
                            Ok(resp) => crate::problem::normalize_error_response(resp).await,
                            Err(err) => {
                                crate::problem::normalize_error_response(
                                    crate::problem::render_error(err),
                                )
                                .await
                            }
                        })
                    })
                    .map_to_response()
                    .boxed()
            }
        };

        self.endpoint = Some(endpoint);
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        #[expect(
            clippy::expect_used,
            reason = "the transport lifecycle runs configure before serve"
        )]
        let endpoint = self
            .endpoint
            .expect("HttpTransport::configure must run before serve");
        let bind = self.bind;
        let window = self.shutdown_timeout;
        // poem keeps its connection count private and stops tracking a socket at
        // its upgrade, so the transport counts what it accepts.
        let drain = self.drain;
        let listener = match self.tls {
            Some(tls) => {
                // Checked here: poem's blanket impl on a config stream would accept
                // unusable material, boot healthy and drop every connection.
                let stream = tls
                    .into_rustls_stream()
                    .context("the configured TLS material cannot serve")?;
                tracing::debug!(target: crate::target::HTTP, addr = %bind, tls = true, "transport listening");
                drain.track(TcpListener::bind(bind)).rustls(stream).boxed()
            }
            None => {
                tracing::debug!(target: crate::target::HTTP, addr = %bind, tls = false, "transport listening");
                drain.track(TcpListener::bind(bind)).boxed()
            }
        };
        let detached = self.detached;
        // poem enforces the window; `begin` runs before poem starts that clock, so
        // every socket poem closes at the bound is counted as closed by it.
        let signal = {
            let drain = Arc::clone(&drain);
            let leaving: Vec<DetachedWork> =
                detached.iter().map(|(_, work)| work.clone()).collect();
            async move {
                cancel.cancelled().await;
                drain.begin(window);
                for work in &leaving {
                    work.go_away();
                }
            }
        };
        let served = Server::new(listener)
            .run_with_graceful_shutdown(endpoint, signal, Some(window))
            .await;
        // Last, so nothing a connection carried still runs when the shutdown hooks start.
        DetachedWork::stop_at(&detached, drain.bound()).await;
        if served.is_ok() {
            drain.report(window);
        }
        served?;
        Ok(())
    }

    /// The window, then the settle `serve` spends stopping what its
    /// self-mounts ran off their connections.
    fn stop_bound(&self) -> Duration {
        self.shutdown_timeout + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_path_concatenates_clean_segments() {
        assert_eq!(join_path("/health", "/live"), "/health/live");
        assert_eq!(join_path("/users", "/:id"), "/users/:id");
    }

    #[test]
    fn join_path_strips_redundant_slashes_on_either_side() {
        assert_eq!(join_path("/health/", "/live"), "/health/live");
        assert_eq!(join_path("/health", "live"), "/health/live");
        assert_eq!(join_path("/health/", "live"), "/health/live");
    }

    #[test]
    fn join_path_handles_empty_or_root_segments() {
        assert_eq!(join_path("", ""), "/");
        assert_eq!(join_path("/", ""), "/");
        assert_eq!(join_path("/", "/"), "/");
        assert_eq!(join_path("", "/users"), "/users");
        assert_eq!(join_path("/users", ""), "/users");
    }

    #[test]
    fn version_path_prefixes_when_a_version_is_supplied() {
        assert_eq!(version_path(Some("1"), "/users"), "/v1/users");
        assert_eq!(version_path(Some("2"), "/users/:id"), "/v2/users/:id");
        assert_eq!(version_path(Some("1"), "/"), "/v1");
    }

    #[test]
    fn version_path_leaves_an_unversioned_path_alone() {
        assert_eq!(version_path(None, "/users"), "/users");
        assert_eq!(version_path(None, "/"), "/");
    }

    #[test]
    fn a_literal_mount_path_is_kept_canonical() {
        assert_eq!(literal_mount_path("").as_deref(), Some("/"));
        assert_eq!(literal_mount_path("assets/").as_deref(), Some("/assets"));
        assert_eq!(
            literal_mount_path("/.well-known").as_deref(),
            Some("/.well-known")
        );
        assert_eq!(literal_mount_path("/app/v2").as_deref(), Some("/app/v2"));
    }

    #[test]
    fn a_mount_path_poem_would_read_as_a_pattern_is_refused() {
        for raw in [
            "/assets/*rest",
            "/:id",
            "/files/<\\d+>",
            "/a//b",
            "/a/../b",
            "/a%2fb",
            "/a b",
            "/a?b",
            "/a\\b",
        ] {
            assert_eq!(literal_mount_path(raw), None, "{raw}");
        }
    }

    #[test]
    fn http_transport_defaults_match_an_empty_new() {
        let d = HttpTransport::default();
        let n = HttpTransport::new();
        assert_eq!(d.bind, n.bind);
        assert_eq!(d.bind, "0.0.0.0:3000");
        assert!(d.mounts.is_empty());
        assert!(d.cors.is_none());
        assert!(d.tls.is_none());
        assert!(d.server_header.is_none());
        assert!(d.endpoint.is_none());
        assert_eq!(
            d.shutdown_timeout,
            crate::HttpConfig::default().shutdown_timeout,
            "a transport built by hand gets the window a configured one defaults to",
        );
    }

    #[test]
    fn the_stop_bound_is_the_configured_window_then_the_settle() {
        let window = Duration::from_secs(3);
        let t = HttpTransport::new().shutdown_timeout(window);
        assert_eq!(
            t.stop_bound(),
            window + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT
        );
    }

    #[test]
    fn bind_overrides_the_default_address() {
        let t = HttpTransport::new().bind("127.0.0.1:9000");
        assert_eq!(t.bind, "127.0.0.1:9000");
    }

    #[test]
    fn tls_pins_the_supplied_config() {
        let t = HttpTransport::new().tls(HttpTls::new(b"cert".to_vec(), b"key".to_vec()));
        assert!(t.tls.is_some());
    }

    #[test]
    fn server_header_pins_the_supplied_static_str() {
        let t = HttpTransport::new().server_header("nestrs/0.1.0");
        assert_eq!(t.server_header, Some("nestrs/0.1.0"));
    }

    #[test]
    fn take_endpoint_returns_none_before_configure_has_run() {
        let mut t = HttpTransport::new();
        assert!(t.take_endpoint().is_none(), "no endpoint before configure");
    }
}
