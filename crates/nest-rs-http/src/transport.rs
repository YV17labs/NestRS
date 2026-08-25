use std::collections::HashMap;

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
use crate::endpoint::{EdgePosture, HttpEndpointMeta, SelfMountGuardWrap};
use crate::interceptor::HttpEndpointWrap;
use crate::tls::TlsConfig;
use crate::versioning::VersionedEndpoint;

type MountFn = Box<dyn Fn(&Container, Route) -> Route + Send + Sync>;
/// Imperative mount paired with its path — kept so the fail-secure boot
/// check can name the endpoints that bypass the layer pool.
type NamedMount = (String, MountFn);

/// Join a controller prefix with a route path the way poem's nesting does:
/// `("/health", "/live") -> "/health/live"`. Public so `nest-rs-openapi`
/// composes paths identically to how this transport mounts them — the served
/// path and the documented path must not drift.
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

/// Apply URI API versioning: `Some("1"), "/users"` → `"/v1/users"`. The single
/// place the URI strategy lives — `#[routes]`, the boot route log, and the
/// OpenAPI document all route through it so the served/logged/documented paths
/// can never drift.
pub fn version_path(version: Option<&str>, path: &str) -> String {
    match version {
        Some(v) => join_path(&format!("/v{v}"), path),
        None => path.to_string(),
    }
}

/// Does `declared` contain every version in `named`?
///
/// `#[routes]` emits this as a `const` assertion per `#[version("…")]` route,
/// because the alternative is worse than a bad error message: the mount loops
/// over the controller's versions, so a route naming one the controller never
/// declared would simply never be reached by the loop — a handler that compiles,
/// registers, documents itself and answers nothing. Silence on a typo is the one
/// outcome this framework does not ship.
///
/// A `const fn` rather than a boot check so the answer arrives where the mistake
/// is, at the route.
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

/// HTTP [`Transport`] backed by poem. At [`Transport::configure`] time, runs
/// every discovered [`HttpBootCheck`], mounts every
/// `#[module(providers = [...])]`-declared [`HttpControllerMeta`] and
/// [`HttpEndpointMeta`], then any imperative [`HttpTransport::mount`], then
/// folds every discovered [`HttpEndpointWrap`] wrap around the assembled
/// endpoint. Transport-edge wraps (the global interceptor / filter pools,
/// infra `#[interceptor]`s like `DbContext`) attach themselves through
/// [`HttpEndpointWrap`] from their own crates — this transport stays free
/// of the cross-transport trait crates and only knows about poem. Guards
/// and pipes never wrap here: they execute in the per-route shaper
/// (post-routing) or at a `Guarded` self-mount's edge.
pub struct HttpTransport {
    bind: String,
    mounts: Vec<NamedMount>,
    cors: Option<Cors>,
    tls: Option<TlsConfig>,
    server_header: Option<&'static str>,
    global_prefix: Option<String>,
    max_body_bytes: Option<usize>,
    request_timeout: Option<std::time::Duration>,
    fail_secure_strict: bool,
    security_headers: crate::SecurityHeadersConfig,
    compression: bool,
    version_selector: Option<crate::VersionSelector>,
    endpoint: Option<BoxEndpoint<'static, Response>>,
}

/// Normalize a global prefix: trim whitespace, drop empty/`"/"` to `None`,
/// prepend a leading `/`, strip a trailing one. `Some("/api/v1")` is the
/// canonical form.
fn normalize_global_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("/{trimmed}"))
}

/// The canonical form of a mount path: `"/x"`, with the root as `"/"`.
///
/// **A mount path is compared as a string before it is served.**
/// `claim_exclusive_path` and the cross-family check key on it to report a
/// collision by owner, and `Route::nest` appends a trailing `/` internally — so
/// `"/x"` and `"/x/"` are two distinct owners here and one key inside poem. Left
/// raw they pass the check that exists to catch exactly that, and poem panics
/// during route assembly instead. Applied by [`HttpEndpointMeta::new`], so every
/// self-mount — MCP, GraphQL (whose path is `NESTRS_GRAPHQL__PATH`, i.e.
/// deployment input), WS, OpenAPI — is canonical before anything compares it.
pub fn normalize_mount_path(raw: &str) -> String {
    match normalize_global_prefix(raw) {
        Some(path) => path,
        None => "/".to_owned(),
    }
}

/// Claim `path` for `owner`, or fail boot naming both claimants.
///
/// A controller prefix and a self-mounted endpoint path are one rule in two
/// vocabularies: each `nest`s under its own path, so two mounts sharing one make
/// poem panic deep in route assembly (`duplicate path: <prefix>/*--poem-rest`).
/// Both callers claim through here so the two boot diagnostics stay worded alike
/// by construction, and neither reaches the opaque poem internal.
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

/// What to do about two controllers claiming one mount prefix. The remedy is
/// not the same sentence in both cases, and saying "give each one a distinct
/// path" to someone running two versions of one resource is advice against the
/// layout the docs prescribe: their paths are identical *by design*, and the
/// string in the message (`/v2/posts`) is one neither of them wrote.
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
    /// A transport with framework defaults — bind `0.0.0.0:3000`, no TLS/CORS,
    /// fail-secure strict. [`HttpModule`](crate::HttpModule) configures it from
    /// [`HttpConfig`](crate::HttpConfig); apps rarely build it directly.
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
            // Fail-secure by default: when global guards are active, an
            // endpoint the transport cannot shape fails boot instead of
            // mounting unguarded. Opt out via `fail_secure_strict(false)` /
            // `NESTRS_HTTP__FAIL_SECURE_STRICT=false`.
            fail_secure_strict: true,
            security_headers: crate::SecurityHeadersConfig::default(),
            compression: false,
            // `None` is the URI strategy: the version is already in the path a
            // controller mounts at, so there is nothing to resolve per request.
            version_selector: None,
            endpoint: None,
        }
    }

    /// Build the transport an [`HttpConfig`](crate::HttpConfig) describes.
    ///
    /// The one place a config becomes a transport. `HttpModule`'s
    /// `TransportContribution` calls it, and so does `nest_rs_testing::TestApp`
    /// — which is the point: a harness that built its own bare transport was
    /// asserting against something the deployment never runs, silently ignoring
    /// the global prefix, the versioning strategy, the body cap, the timeout,
    /// CORS and the security headers.
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
        // Install the per-request cap as a request-data entry — the `RawBody`
        // extractor reads it back from the extensions.
        http = http.max_body_bytes(cfg.max_body_bytes.unwrap_or(crate::RawBody::DEFAULT_LIMIT));
        if let Some(timeout) = cfg.request_timeout {
            http = http.request_timeout(timeout);
        }
        http = http.fail_secure_strict(cfg.fail_secure_strict);
        http = http.security_headers(cfg.security_headers.clone());
        http = http.compression(cfg.compression);
        // `trusted_proxies` is deliberately not handed to the transport: it is a
        // boot-time constant, so `ClientOrigin` reads it off the `HttpConfig` in
        // the container rather than through per-request state.
        Ok(http)
    }

    /// Resolve each request's API version through `selector` instead of from
    /// its path. [`HttpModule`](crate::HttpModule) passes what `HttpConfig`
    /// describes; the URI strategy passes nothing.
    pub fn api_versioning(mut self, selector: crate::VersionSelector) -> Self {
        self.version_selector = Some(selector);
        self
    }

    /// Pin the default security-header policy. [`HttpModule`](crate::HttpModule)
    /// passes `HttpConfig.security_headers`; defaults are safe (nosniff +
    /// `X-Frame-Options: DENY` + HSTS under TLS).
    pub fn security_headers(mut self, cfg: crate::SecurityHeadersConfig) -> Self {
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

    /// Mount every controller under a shared prefix (e.g. `/api`). Useful
    /// behind a reverse proxy that hands off a sub-path. Empty / `"/"`
    /// collapse to no-op; a missing leading `/` is added; a trailing `/` is
    /// stripped.
    pub fn global_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.global_prefix = normalize_global_prefix(&prefix.into());
        self
    }

    /// Emit `Server: <value>` on every response — off by default
    /// (production-safe). [`HttpModule`](crate::HttpModule) sets this when
    /// `HttpConfig.server_header` is `true`, using `nestrs/<crate version>`.
    pub fn server_header(mut self, value: &'static str) -> Self {
        self.server_header = Some(value);
        self
    }

    /// Set the listen address (`host:port`).
    pub fn bind(mut self, addr: impl Into<String>) -> Self {
        self.bind = addr.into();
        self
    }

    /// Cap each request's raw body to `limit` bytes. Read back by the
    /// [`RawBody`](crate::RawBody) extractor via the ambient request
    /// context ([`current_body_limit`](crate::current_body_limit)).
    pub fn max_body_bytes(mut self, limit: usize) -> Self {
        self.max_body_bytes = Some(limit);
        self
    }

    /// Abort any request that runs longer than `timeout`, answering the client
    /// with `503 Service Unavailable` and a `Retry-After`. Bounds connection
    /// hold time against slow or stuck handlers. Without this call no timeout is
    /// enforced.
    pub fn request_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// Enable CORS with a configured poem [`Cors`] middleware. Wraps the route
    /// tree outermost so a preflight (`OPTIONS`) is answered before any guard
    /// or interceptor runs.
    pub fn cors(mut self, cors: Cors) -> Self {
        self.cors = Some(cors);
        self
    }

    /// Negotiate response compression from each request's `Accept-Encoding`
    /// (poem's [`Compression`] middleware — gzip / deflate / brotli / zstd).
    /// Off by default; [`HttpModule`](crate::HttpModule) turns it on when
    /// `HttpConfig.compression` is set.
    pub fn compression(mut self, on: bool) -> Self {
        self.compression = on;
        self
    }

    /// Serve HTTPS directly from [`TlsConfig`] (poem's `rustls` listener)
    /// instead of plain HTTP. Without this call the transport stays plaintext.
    pub fn tls(mut self, tls: TlsConfig) -> Self {
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
                let endpoint = build(container).into_endpoint().map_to_response().boxed();
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
        // Boot checks first — a misconfigured global layer pool (a spec whose
        // provider was never registered) must fail boot before anything
        // mounts; resolved-at-configure means dropped-silently otherwise.
        for d in discovery.meta::<HttpBootCheck>() {
            d.meta.run(container).map_err(|msg| anyhow::anyhow!(msg))?;
        }
        let mut route = Route::new();

        // A global guard pool shapes every controller route (it runs post-routing
        // on all of them), so per-route coverage only matters when no pool is
        // registered — then a route is covered iff it declares a controller/method
        // guard or is explicitly `#[public]`. Anything else is an *implicit*
        // access decision: fail-secure asks the developer to make it explicit.
        let global_guards = container.get::<GlobalGuardsActive>().is_some();
        let mut unguarded: Vec<String> = Vec::new();
        // Each controller `nest`s under its own prefix, so a prefix is a
        // controller's exclusive namespace — two controllers sharing one make
        // poem panic deep in route assembly ("duplicate path: <prefix>/*--poem-rest").
        // Catch it here instead, naming both controllers, so it reads like every
        // other nestrs boot failure rather than an opaque poem internal.
        let mut prefix_owner: HashMap<String, String> = HashMap::new();
        // Controllers mount their routes FLAT (`.at("<prefix>/<path>")`), so
        // two controllers whose prefix+path combine to the same full path
        // would hit poem's opaque duplicate-path panic even though their
        // prefixes differ. Claim full paths too — same-controller duplicates
        // (several verbs on one path) are legal and share one entry.
        let mut route_owner: HashMap<String, String> = HashMap::new();

        // Which prefixes carry a version, for the non-URI strategies. Collected
        // here rather than guessed per request: a rewrite that fired on every
        // path would send `/graphql`, `/mcp` and `/health` to `/v1/…` the
        // moment a deployment named a default version.
        // Routes, not controller prefixes: a prefix of `/` matches nothing
        // segment-wise and a prefix of `/posts` matches a *different*
        // controller's `/posts/drafts`. Both were real defects; the question
        // that was always meant is "does a versioned route answer here".
        let mut versioned_routes: Vec<String> = Vec::new();
        // Addresses answered with no version, kept apart because they yield
        // differently: a self-mount is neutral against anything, an unversioned
        // controller route only against a *default* version.
        let mut self_mounts: Vec<String> = Vec::new();
        let mut unversioned_routes: Vec<String> = Vec::new();
        for d in discovery.meta::<HttpControllerMeta>() {
            for version in d.meta.mounted_versions() {
                let prefix = d.meta.effective_prefix(version);
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
                    // Log the address a *client* uses. Under a non-URI strategy
                    // the `/v{n}` prefix is where the route is mounted, not where
                    // it is called, so the version moves out of the path and into
                    // its own field rather than teaching the log a URL nobody can
                    // request.
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

        // A `DEFAULT_VERSION` naming a version nothing declares is a silent
        // misconfiguration, and a bad one: every caller that states no version
        // resolves to a path that does not exist, falls through, and is served
        // the unversioned route or a 404 — with nothing said. Refuse at boot,
        // naming the versions that do exist.
        //
        // Here rather than in `nest-rs-openapi`, which had this check first: it
        // is HTTP's config, and an app that publishes no document deserves the
        // same answer as one that does.
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
        // Provided by `use_guards_global` (which can see the `Guard` trait);
        // absent when no global guard is registered. Applied below to every
        // `Guarded` self-mount — they have no per-route shaper to carry the
        // global guard pool, so the transport runs it at their edge.
        let self_mount_guard = discovery
            .meta::<SelfMountGuardWrap>()
            .into_iter()
            .next()
            .map(|d| d.meta);
        // A `Guarded` self-mount (a WS gateway upgrade) expects the transport
        // to run the global guard chain at its edge; with no global guard pool
        // that chain is empty — the self-mount analog of an implicitly-accessible
        // controller route (the scan above). The gateway may still bind its own
        // `#[use_guards]` inside its opaque mount closure, so this is a boot
        // diagnostic to confirm the edge is guarded on purpose, not a
        // fail-secure stop (the `Guarded` posture already gets the pool wrap
        // below whenever one exists).
        let mut unguarded_edges: Vec<String> = Vec::new();
        // Same exclusivity rule as a controller prefix, same failure mode: two
        // self-mounts on one path make poem panic in route assembly. Catch it
        // here so a second `#[mcp(path = "/mcp")]` reads as a named boot error
        // naming both endpoints, not an opaque poem internal.
        let mut endpoint_owner: HashMap<String, String> = HashMap::new();
        for d in discovery.meta::<HttpEndpointMeta>() {
            // Cross-family too: a self-mount nests its whole subtree, so a
            // controller already holding that path is the same poem panic by
            // another route. The two maps above only ever compared a family
            // against itself, which let `#[controller(path = "/chat")]` beside
            // `#[gateway(path = "/chat")]` through to route assembly.
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
            // A self-mount owns the paths it declares, and owns them without a
            // version: `/graphql`, `/mcp`, `/api-json`, a gateway. Recording
            // them here is what stops a versioned catch-all controller from
            // swallowing them.
            //
            // Every declared path, not the path plus an assumed `/*rest`
            // subtree: the assumption was wrong in both directions at once. It
            // missed `/api-json` — which `OpenApiModule` mounts and which is not
            // under `/api` — so a root catch-all swallowed the document; and it
            // claimed the whole subtree under every self-mount, so a versioned
            // controller mounted beneath one was unreachable with no boot error
            // to say why. A surface that genuinely owns a subtree now says so,
            // through `also_mounts`.
            for path in d.meta.paths() {
                self_mounts.push(path.to_owned());
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
            if d.meta.edge_access_is_implicit(global_guards) {
                unguarded_edges.push(format!("{} ({})", d.meta.path(), d.meta.label()));
            }
            match (d.meta.posture(), &self_mount_guard) {
                (EdgePosture::Guarded, Some(wrap)) => {
                    // Isolate this self-mount into a fresh sub-route, wrap it
                    // with the global guard chain, and nest it back without
                    // stripping its own path (so the inner route still matches).
                    let isolated: BoxEndpoint<'static, Response> =
                        d.meta.mount(container, Route::new()).boxed();
                    let wrapped = wrap.apply(container, isolated);
                    route = route.nest_no_strip(d.meta.path(), wrapped);
                }
                _ => {
                    // `Exempt` surfaces gate in-band (GraphQL operation guard,
                    // MCP per-request guard) or are deliberately public
                    // (OpenAPI docs) — no edge wrap.
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
        // Fail-secure completeness check: every controller route is shaped
        // (its `RouteShaper` runs the global guard pool) and every self-mount
        // declares an `EdgePosture`, but an imperative `mount(...)` is an
        // opaque poem endpoint the transport can neither shape nor introspect.
        // When global guards are active, those endpoints bypass the pool —
        // strict mode (the default) fails boot, the same posture as the
        // access graph; opting out downgrades to a warn.
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
        for (_, mount) in self.mounts.drain(..) {
            route = mount(container, route);
        }

        // Header / media-type versioning is a rewrite in front of routing:
        // fold it back into a `Route` at the root so the no-layer fast path
        // below stays monomorphized, and keep it *inside* the global prefix so
        // the path it rewrites is the one controllers mount at.
        // `rewrites()` and not merely `Some`: the URI strategy is resolved by
        // routing, so wrapping it would refuse every `/v{n}/…` path it is
        // supposed to serve. Unreachable through `HttpModule`, which hands over
        // `None` for `uri` — the public builder can hit it.
        if let Some(selector) = self.version_selector.take().filter(|s| s.rewrites()) {
            let selector = selector.with_routes(
                versioned_routes,
                self_mounts,
                unversioned_routes,
                crate::declared_versions(container),
            );
            // An inert selector — a non-URI strategy with nothing versioned to
            // select — would pass every request straight through, at the cost of
            // a whole extra routing layer. Measured at +57% on the hot path, for
            // an outcome it can never change.
            if !selector.is_inert() {
                route = Route::new().nest_no_strip("/", VersionedEndpoint::new(route, selector));
            }
        }

        // Apply the global prefix once around the fully-assembled tree so
        // every controller, every self-mounting endpoint, and every imperative
        // `mount(...)` lands under it.
        if let Some(prefix) = self.global_prefix.take() {
            route = Route::new().nest(prefix, route);
        }

        // Layer-System globals (guards / interceptors / filters / pipes /
        // exception filters) attach a `HttpEndpointWrap` from their own
        // crate. The transport sorts by priority ascending so the
        // documented HTTP order is enforced regardless of AppBuilder call
        // sequence: Guards (innermost) → Filters → Interceptors
        // (outermost). Insertion order is the tiebreaker within a band.
        let mut metas: Vec<std::sync::Arc<HttpEndpointWrap>> = discovery
            .meta::<HttpEndpointWrap>()
            .into_iter()
            .map(|d| d.meta)
            .collect();
        metas.sort_by_key(|m| m.priority());
        // The four per-request edge concerns — request scope, body cap,
        // request timeout, default response headers (security + `Server`) —
        // fuse into ONE layer (`EdgeEndpoint`) instead of one boxed wrap
        // each: same semantics and relative order (scope outermost, body cap
        // before the timer, the timer bounding guards/interceptors/handler,
        // headers stamped on the way out — so a `413`/`503` still carries
        // them), a single dispatch on the hot path. CORS / compression stay
        // poem middlewares outside it; a preflight is answered before any of
        // this runs, and without the timer.
        let mut edge_headers: Vec<(HeaderName, HeaderValue)> = Vec::new();
        for (name, value) in self.security_headers.headers(self.tls.is_some()) {
            // Values are boot-validated (HTTP-S4) and the names are the `http`
            // crate's own constants, so a failure here is a framework bug, not
            // a config error — log it loudly rather than silently drop a
            // security header.
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
        // Transport-edge error boundary — outermost, so it normalizes
        // whatever escapes the whole stack. Any `>= 400` response poem
        // rendered as raw `text/plain` (an unmounted-route 404, a 413, a 405,
        // an extractor's bad-path-id 400, a timeout 503) is lifted onto the
        // single RFC-9457 `application/problem+json` envelope; a response
        // already in `problem+json` (a `ServiceError`, a `ProblemDetails`, a
        // guard denial, a domain exception filter) passes through untouched.
        // `map_to_response` on the route tree collapses handler/extractor
        // `Err`s into `Ok` responses before here, so the seam inspects the
        // response, not `Err`. Without CORS / compression the edge is the
        // outermost layer and runs the normalizer itself; with them the
        // boundary stays a separate wrap outside both.
        let fuse_normalize = !self.compression && self.cors.is_none();
        let timeout = self.request_timeout.take();
        let body_limit = self.max_body_bytes.take();

        let endpoint: BoxEndpoint<'static, Response> = if metas.is_empty() && fuse_normalize {
            // Fast path — no global wrap, no CORS, no compression: the edge
            // sits directly on the (unboxed) route tree, monomorphized, and
            // the whole transport stack is a single boxed endpoint.
            crate::edge::EdgeEndpoint::new(
                route.map_to_response(),
                container.clone(),
                timeout,
                body_limit,
                edge_headers,
                true,
                // This shape is only mounted when neither CORS nor compression
                // is configured, so nothing outside can rewrite the body.
                false,
            )
            .boxed()
        } else {
            // Render whatever is still an `Err` into its response once the
            // global filter pool has had its turn, so the interceptor bands
            // above genuinely see 404s and 405s (the router answers an
            // unmatched path with `Err`, which short-circuits the documented
            // `next.run(req).await?` body).
            //
            // `metas` is sorted ascending, so the insertion point is a
            // partition — and `to_response` subsumes `map_to_response`, so when
            // nothing sits below the band (the common case: an app with only
            // the interceptor pool and infra wraps) the resolution is *free*,
            // folded into the base layer instead of stacked on top of it.
            // Kept out of the meta list so it stays un-registerable.
            let split = metas
                .partition_point(|m| m.priority() < crate::endpoint_wrap_priority::ERROR_RESOLVE);
            let (below, above) = metas.split_at(split);
            let mut endpoint: BoxEndpoint<'static, Response> = if below.is_empty() {
                route.to_response().boxed()
            } else {
                let mut inner: BoxEndpoint<'static, Response> = route.map_to_response().boxed();
                for meta in below {
                    inner = meta.wrap(container, inner);
                }
                inner.to_response().boxed()
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
                // Compression wraps outside this endpoint and replaces the
                // request body without touching `Content-Length`, so a declared
                // length stops bounding anything.
                self.compression,
            )
            .boxed();
            // Response compression, negotiated from `Accept-Encoding`. Inside
            // CORS (a preflight carries no body to compress) and outside the
            // handler / header layers so the encoded bytes are what leaves
            // the process.
            if self.compression {
                endpoint = endpoint.with(Compression::new()).map_to_response().boxed();
            }
            // CORS wraps outermost, so a preflight is handled before guards
            // run — and before the edge layer, so a preflight carries no
            // request scope (nothing reads one: no extractor or guard runs on
            // a preflight).
            if let Some(cors) = self.cors.take() {
                endpoint = endpoint.with(cors).map_to_response().boxed();
            }
            if fuse_normalize {
                endpoint
            } else {
                endpoint
                    .around(|ep, req| async move {
                        let resp = match ep.call(req).await {
                            Ok(resp) => resp,
                            Err(err) => err.into_response(),
                        };
                        Ok(crate::problem::normalize_error_response(resp).await)
                    })
                    .map_to_response()
                    .boxed()
            }
        };

        self.endpoint = Some(endpoint);
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        let endpoint = self
            .endpoint
            .expect("HttpTransport::configure must run before serve");
        let bind = self.bind;
        let listener = match self.tls {
            Some(tls) => {
                // Built before the listener binds, and fallible on purpose: a
                // stream of configs takes poem's unchecked blanket impl, so
                // unusable material would otherwise leave a process that boots,
                // reports healthy, binds and drops every connection.
                let stream = tls
                    .into_rustls_stream()
                    .context("the configured TLS material cannot serve")?;
                tracing::debug!(target: crate::target::HTTP, addr = %bind, tls = true, "transport listening");
                TcpListener::bind(bind).rustls(stream).boxed()
            }
            None => {
                tracing::debug!(target: crate::target::HTTP, addr = %bind, tls = false, "transport listening");
                TcpListener::bind(bind).boxed()
            }
        };
        Server::new(listener)
            .run_with_graceful_shutdown(endpoint, async move { cancel.cancelled().await }, None)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `join_path` is the single source of truth shared with `nest-rs-openapi`
    // and the boot route log — a drift here means the served path and the
    // documented path disagree, so the cases are exhaustive on purpose.
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
        // Version + root.
        assert_eq!(version_path(Some("1"), "/"), "/v1");
    }

    #[test]
    fn version_path_leaves_an_unversioned_path_alone() {
        assert_eq!(version_path(None, "/users"), "/users");
        assert_eq!(version_path(None, "/"), "/");
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
    }

    #[test]
    fn bind_overrides_the_default_address() {
        let t = HttpTransport::new().bind("127.0.0.1:9000");
        assert_eq!(t.bind, "127.0.0.1:9000");
    }

    #[test]
    fn tls_pins_the_supplied_config() {
        // TlsConfig is opaque, so just check the option flips on.
        let t = HttpTransport::new().tls(TlsConfig::new(b"cert".to_vec(), b"key".to_vec()));
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
