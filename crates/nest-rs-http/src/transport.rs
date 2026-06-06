use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use nest_rs_core::{Container, DiscoveryService, Transport};
use nest_rs_middleware::{EndpointExt as NestrsEndpointExt, Filter, Guard, Interceptor};
use poem::endpoint::BoxEndpoint;
use poem::http::header::{HeaderValue, SERVER};
use poem::listener::{Listener, TcpListener};
use poem::middleware::{Cors, SetHeader};
use poem::{EndpointExt, IntoEndpoint, Response, Route, Server};
use tokio_util::sync::CancellationToken;

use crate::controller::HttpControllerMeta;
use crate::endpoint::HttpEndpointMeta;
use crate::interceptor::HttpInterceptorMeta;
use crate::tls::TlsConfig;

type MountFn = Box<dyn Fn(&Container, Route) -> Route + Send + Sync>;

/// Join a controller prefix with a route path the way poem's nesting does:
/// `("/health", "/live") -> "/health/live"`. Public so `nestrs-openapi`
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

/// HTTP [`Transport`] backed by poem. At [`Transport::configure`] time, mounts
/// every `#[module(providers = [...])]`-declared [`HttpControllerMeta`] and
/// [`HttpEndpointMeta`], then any imperative [`HttpTransport::mount`], then
/// folds the interceptor / guard / filter chain around the assembled route.
pub struct HttpTransport {
    bind: String,
    interceptors: Vec<Arc<dyn Interceptor>>,
    guards: Vec<Arc<dyn Guard>>,
    filters: Vec<Arc<dyn Filter>>,
    mounts: Vec<MountFn>,
    cors: Option<Cors>,
    tls: Option<TlsConfig>,
    server_header: Option<&'static str>,
    global_prefix: Option<String>,
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

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport {
    pub fn new() -> Self {
        Self {
            bind: "0.0.0.0:3000".into(),
            interceptors: Vec::new(),
            guards: Vec::new(),
            filters: Vec::new(),
            mounts: Vec::new(),
            cors: None,
            tls: None,
            server_header: None,
            global_prefix: None,
            endpoint: None,
        }
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

    pub fn bind(mut self, addr: impl Into<String>) -> Self {
        self.bind = addr.into();
        self
    }

    pub fn interceptor<I: Interceptor>(mut self, interceptor: I) -> Self {
        self.interceptors.push(Arc::new(interceptor));
        self
    }

    pub fn guard<G: Guard>(mut self, guard: G) -> Self {
        self.guards.push(Arc::new(guard));
        self
    }

    pub fn filter<F: Filter>(mut self, filter: F) -> Self {
        self.filters.push(Arc::new(filter));
        self
    }

    /// Enable CORS with a configured poem [`Cors`] middleware. Wraps the route
    /// tree outermost so a preflight (`OPTIONS`) is answered before any guard
    /// or interceptor runs.
    pub fn cors(mut self, cors: Cors) -> Self {
        self.cors = Some(cors);
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
        self.mounts.push(Box::new(move |container, route| {
            let endpoint = build(container).into_endpoint().map_to_response().boxed();
            route.nest(path.clone(), endpoint)
        }));
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
        let mut route = Route::new();

        for d in discovery.meta::<HttpControllerMeta>() {
            let prefix = d.meta.effective_prefix();
            for r in &d.meta.routes {
                tracing::info!(
                    target: "nest_rs::routes",
                    "{:<6} {}  ({})",
                    r.verb.as_str(),
                    join_path(&prefix, r.path),
                    r.handler,
                );
            }
            route = d.meta.mount(container, route);
        }
        for d in discovery.meta::<HttpEndpointMeta>() {
            tracing::info!(
                target: "nest_rs::routes",
                "{:<6} {}  ({})",
                "*",
                d.meta.path(),
                d.meta.label(),
            );
            route = d.meta.mount(container, route);
        }
        for mount in self.mounts.drain(..) {
            route = mount(container, route);
        }

        // Apply the global prefix once around the fully-assembled tree so
        // every controller, every self-mounting endpoint, and every imperative
        // `mount(...)` lands under it.
        if let Some(prefix) = self.global_prefix.take() {
            route = Route::new().nest(prefix, route);
        }

        let mut endpoint: BoxEndpoint<'static, Response> = route.map_to_response().boxed();
        for filter in self.filters.drain(..) {
            endpoint = NestrsEndpointExt::filter(endpoint, filter)
                .map_to_response()
                .boxed();
        }
        for guard in self.guards.drain(..) {
            endpoint = NestrsEndpointExt::guard(endpoint, guard)
                .map_to_response()
                .boxed();
        }
        for d in discovery.meta::<HttpInterceptorMeta>() {
            endpoint = NestrsEndpointExt::interceptor(endpoint, d.meta.interceptor())
                .map_to_response()
                .boxed();
        }
        for interceptor in self.interceptors.drain(..) {
            endpoint = NestrsEndpointExt::interceptor(endpoint, interceptor)
                .map_to_response()
                .boxed();
        }
        // Server header is purely cosmetic — apply before CORS so the
        // preflight short-circuit (no body) still carries it for observability.
        if let Some(value) = self.server_header.take() {
            let header_value = HeaderValue::from_static(value);
            let set = SetHeader::new().overriding(SERVER, header_value);
            endpoint = endpoint.with(set).map_to_response().boxed();
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
                tracing::info!(addr = %bind, "https transport listening (TLS)");
                TcpListener::bind(bind).rustls(tls.into_rustls()).boxed()
            }
            None => {
                tracing::info!(addr = %bind, "http transport listening");
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

    // `join_path` is the single source of truth shared with `nestrs-openapi`
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
        assert!(d.interceptors.is_empty());
        assert!(d.guards.is_empty());
        assert!(d.filters.is_empty());
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
