use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use nest_rs_core::{Container, DiscoveryService, Transport};
use poem::endpoint::BoxEndpoint;
use poem::http::header::{HeaderName, HeaderValue, SERVER};
use poem::listener::{Listener, TcpListener};
use poem::middleware::{Compression, Cors, SetHeader};
use poem::{EndpointExt, IntoEndpoint, Response, Route, Server};
use tokio_util::sync::CancellationToken;

use crate::boot_check::{GlobalGuardsActive, HttpBootCheck};
use crate::controller::HttpControllerMeta;
use crate::endpoint::{EdgePosture, HttpEndpointMeta, SelfMountGuardWrap};
use crate::interceptor::HttpEndpointWrap;
use crate::raw_body::RawBodyLimit;
use crate::tls::TlsConfig;

type MountFn = Box<dyn Fn(&Container, Route) -> Route + Send + Sync>;
/// Imperative mount paired with its path — kept so the fail-secure boot
/// check can name the endpoints that bypass the layer pool.
type NamedMount = (String, MountFn);

/// A bare `413 Payload Too Large` — the transport-edge body cap's rejection.
fn payload_too_large() -> Response {
    Response::builder()
        .status(poem::http::StatusCode::PAYLOAD_TOO_LARGE)
        .finish()
}

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
) -> anyhow::Result<()> {
    if let Some(first) = owners.insert(path.clone(), owner.clone()) {
        anyhow::bail!(
            "duplicate {kind} {path:?}: {first} and {owner} both mount there — a {kind} is its \
             exclusive namespace; give each one a distinct path",
        );
    }
    Ok(())
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
            endpoint: None,
        }
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
    /// [`RawBody`](crate::RawBody) extractor via the
    /// [`RawBodyLimit`](crate::RawBodyLimit) request extension.
    pub fn max_body_bytes(mut self, limit: usize) -> Self {
        self.max_body_bytes = Some(limit);
        self
    }

    /// Abort any request that runs longer than `timeout`, answering the client
    /// with `504 Gateway Timeout`. Bounds connection hold time against slow or
    /// stuck handlers. Without this call no timeout is enforced.
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
        let discovery = DiscoveryService::new(container);
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

        for d in discovery.meta::<HttpControllerMeta>() {
            let prefix = d.meta.effective_prefix();
            claim_exclusive_path(
                &mut prefix_owner,
                "controller prefix",
                prefix.clone(),
                d.meta.controller.to_owned(),
            )?;
            for r in &d.meta.routes {
                let path = join_path(&prefix, r.path);
                tracing::info!(
                    target: "nest_rs::routes",
                    controller = d.meta.controller,
                    method = r.verb.as_str(),
                    path = path.as_str(),
                    handler = r.handler,
                    "mounted route",
                );
                if r.access_is_implicit(global_guards) {
                    unguarded.push(format!("{} {} ({})", r.verb.as_str(), path, r.handler));
                }
            }
            route = d.meta.mount(container, route);
        }

        if !unguarded.is_empty() {
            tracing::warn!(
                target: "nest_rs::layers",
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
            claim_exclusive_path(
                &mut endpoint_owner,
                "self-mounted endpoint path",
                d.meta.path().to_owned(),
                format!("a {} endpoint", d.meta.label()),
            )?;
            tracing::info!(
                target: "nest_rs::routes",
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
                target: "nest_rs::layers",
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
                     with HttpTransport::fail_secure_strict(false) / \
                     NESTRS_HTTP__FAIL_SECURE_STRICT=false",
                    paths.join(", "),
                );
            }
            tracing::warn!(
                target: "nest_rs::http",
                paths = paths.join(", ").as_str(),
                hint = "route through a #[controller] or guard explicitly",
                "imperative mounts bypass the global guard pool",
            );
        }
        for (_, mount) in self.mounts.drain(..) {
            route = mount(container, route);
        }

        // Apply the global prefix once around the fully-assembled tree so
        // every controller, every self-mounting endpoint, and every imperative
        // `mount(...)` lands under it.
        if let Some(prefix) = self.global_prefix.take() {
            route = Route::new().nest(prefix, route);
        }

        let mut endpoint: BoxEndpoint<'static, Response> = route.map_to_response().boxed();
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
        for meta in metas {
            endpoint = meta.wrap(container, endpoint);
        }
        // Wrap the whole Layer System in a wall-clock budget: a handler that
        // overruns is aborted and the client gets `504`. Outside the globals
        // so guards/interceptors are themselves bounded; inside body-limit /
        // header / CORS so a preflight is still answered without the timer.
        if let Some(timeout) = self.request_timeout.take() {
            endpoint = endpoint
                .around(move |ep, req| async move {
                    match tokio::time::timeout(timeout, ep.call(req)).await {
                        Ok(res) => res,
                        Err(_) => {
                            tracing::warn!(target: "nest_rs::http", ?timeout, "request timed out");
                            Ok(Response::builder()
                                .status(poem::http::StatusCode::GATEWAY_TIMEOUT)
                                .finish())
                        }
                    }
                })
                .map_to_response()
                .boxed();
        }
        // Enforce the body-byte cap at the transport edge so EVERY extractor —
        // a bare `Json`/`String`/`Vec<u8>`/`Multipart`, not just `RawBody` —
        // sits under it (B-HTTP-2). Two gates: a fast `413` on an oversized
        // declared `Content-Length` (rejects before reading a byte), then a
        // streaming cap that buffers at most `limit` bytes and rejects a body
        // that runs past it (covers `Transfer-Encoding: chunked` and a client
        // lying about `Content-Length`). The `RawBodyLimit` data extension stays
        // installed OUTSIDE the Layer System globals so every interceptor /
        // filter / guard still sees the configured value, and `RawBody` /
        // `extract_with_limit` can pin a *tighter* per-route cap under this
        // ceiling.
        if let Some(limit) = self.max_body_bytes.take() {
            endpoint = endpoint
                .around(move |ep, mut req| async move {
                    use poem::error::ReadBodyError;
                    use poem::web::headers::{ContentLength, HeaderMapExt};
                    if let Some(ContentLength(declared)) =
                        req.headers().typed_get::<ContentLength>()
                        && declared as usize > limit
                    {
                        return Ok(payload_too_large());
                    }
                    match req.take_body().into_bytes_limit(limit).await {
                        Ok(bytes) => {
                            req.set_body(bytes);
                            ep.call(req).await
                        }
                        Err(ReadBodyError::PayloadTooLarge) => Ok(payload_too_large()),
                        Err(err) => Err(err.into()),
                    }
                })
                .data(RawBodyLimit(limit))
                .map_to_response()
                .boxed();
        }
        // Default security headers (nosniff / frame-deny / HSTS-under-TLS).
        // Applied inside CORS so a preflight isn't burdened, and overriding so a
        // handler that set one wins is a deliberate exception, not the default.
        let security_headers = self.security_headers.headers(self.tls.is_some());
        if !security_headers.is_empty() {
            let mut set = SetHeader::new();
            for (name, value) in security_headers {
                // Values are boot-validated (HTTP-S4) and names are static, so a
                // failure here is a framework bug, not a config error — log it
                // loudly rather than silently drop a security header.
                match (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(&value),
                ) {
                    (Ok(header_name), Ok(header_value)) => {
                        set = set.overriding(header_name, header_value);
                    }
                    _ => tracing::error!(
                        target: "nest_rs::http",
                        header = name,
                        "failed to construct a security header despite boot validation",
                    ),
                }
            }
            endpoint = endpoint.with(set).map_to_response().boxed();
        }
        // Server header is purely cosmetic — apply before CORS so the
        // preflight short-circuit (no body) still carries it for observability.
        if let Some(value) = self.server_header.take() {
            let header_value = HeaderValue::from_static(value);
            let set = SetHeader::new().overriding(SERVER, header_value);
            endpoint = endpoint.with(set).map_to_response().boxed();
        }
        // Response compression, negotiated from `Accept-Encoding`. Inside CORS
        // (a preflight carries no body to compress) and outside the handler /
        // header layers so the encoded bytes are what leaves the process.
        if self.compression {
            endpoint = endpoint.with(Compression::new()).map_to_response().boxed();
        }
        // CORS wraps outermost, so a preflight is handled before guards run.
        if let Some(cors) = self.cors.take() {
            endpoint = endpoint.with(cors).map_to_response().boxed();
        }
        // Request scope installs before anything else so guards/handlers can
        // resolve `#[injectable(scope = request)]` providers via `Scoped<T>`.
        endpoint = crate::RequestScopeEndpoint::new(endpoint, container.clone())
            .map_to_response()
            .boxed();

        // Transport-edge error boundary — the outermost wrap, so it normalizes
        // whatever escapes the whole stack. Any `>= 400` response poem rendered
        // as raw `text/plain` (an unmounted-route 404, a 413, a 405, an
        // extractor's bad-path-id 400, a timeout 504) is lifted onto the single
        // RFC-9457 `application/problem+json` envelope; a response already in
        // `problem+json` (a `ServiceError`, a `ProblemDetails`, a guard denial,
        // a domain exception filter) passes through untouched. `map_to_response`
        // on the route tree collapses handler/extractor `Err`s into `Ok`
        // responses before here, so the seam inspects the response, not `Err`.
        endpoint = endpoint
            .around(|ep, req| async move {
                let resp = match ep.call(req).await {
                    Ok(resp) => resp,
                    Err(err) => err.into_response(),
                };
                Ok(crate::problem::normalize_error_response(resp).await)
            })
            .map_to_response()
            .boxed();

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
                tracing::debug!(target: "nest_rs::http", addr = %bind, tls = true, "transport listening");
                TcpListener::bind(bind).rustls(tls.into_rustls()).boxed()
            }
            None => {
                tracing::debug!(target: "nest_rs::http", addr = %bind, tls = false, "transport listening");
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
