use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use nestrs_core::{Container, DiscoveryService, Transport};
use nestrs_middleware::{EndpointExt as NestrsEndpointExt, Filter, Guard, Interceptor};
use poem::endpoint::BoxEndpoint;
use poem::listener::{Listener, TcpListener};
use poem::http::header::{HeaderValue, SERVER};
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
    endpoint: Option<BoxEndpoint<'static, Response>>,
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
            endpoint: None,
        }
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
                    target: "nestrs::routes",
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
                target: "nestrs::routes",
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
