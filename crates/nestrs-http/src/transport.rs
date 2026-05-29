use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use nestrs_core::{Container, DiscoveryService, Transport};
use nestrs_middleware::{EndpointExt as NestrsEndpointExt, Filter, Guard, Interceptor};
use poem::endpoint::BoxEndpoint;
use poem::listener::{Listener, TcpListener};
use poem::middleware::Cors;
use poem::{EndpointExt, IntoEndpoint, Response, Route, Server};
use tokio_util::sync::CancellationToken;

use crate::controller::HttpControllerMeta;
use crate::endpoint::HttpEndpointMeta;
use crate::interceptor::HttpInterceptorMeta;
use crate::tls::TlsConfig;

type MountFn = Box<dyn Fn(&Container, Route) -> Route + Send + Sync>;

/// Join a controller prefix with a route path the way `poem`'s nesting does:
/// `("/health", "/live") -> "/health/live"`, `("/", "/") -> "/"`. Public so the
/// OpenAPI document (`nestrs-openapi`) composes paths identically to how this
/// transport mounts them — the two must not drift.
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

/// Apply URI API versioning to a controller path: a declared `version` becomes a
/// `/v{version}` segment in front of `path` (`Some("1"), "/users"` →
/// `"/v1/users"`); an absent version leaves `path` untouched. This is the **one**
/// place the URI strategy lives — `#[routes]` (which mounts), the boot route log,
/// and the OpenAPI document all route through it so the served path, the logged
/// path, and the documented path can never drift. Header / media-type versioning
/// (NestJS's other `VersioningType`s) need request-time dispatch and are not yet
/// implemented; URI versioning covers the common case declaratively.
pub fn version_path(version: Option<&str>, path: &str) -> String {
    match version {
        Some(v) => join_path(&format!("/v{v}"), path),
        None => path.to_string(),
    }
}

/// HTTP [`Transport`] backed by poem. Built up imperatively in the app's
/// `main.rs`, attached to an [`nestrs_core::App`], and configured by it.
///
/// At [`Transport::configure`] time, the transport queries the container's
/// [`DiscoveryService`] for every [`HttpControllerMeta`] and every
/// [`HttpEndpointMeta`] declared via `#[module(providers = [...])]` — the
/// latter is how a GraphQL schema or MCP service mounts itself — then mounts
/// any extra endpoints registered imperatively via [`HttpTransport::mount`],
/// then folds the interceptor / guard / filter chain around the assembled
/// route.
pub struct HttpTransport {
    bind: String,
    interceptors: Vec<Arc<dyn Interceptor>>,
    guards: Vec<Arc<dyn Guard>>,
    filters: Vec<Arc<dyn Filter>>,
    mounts: Vec<MountFn>,
    cors: Option<Cors>,
    tls: Option<TlsConfig>,
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
            endpoint: None,
        }
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

    /// Enable CORS with a configured `poem` [`Cors`] middleware — the analog of
    /// NestJS's `app.enableCors(...)`. Build it from the re-exported poem
    /// (`nestrs_http::poem::middleware::Cors::new().allow_origin("https://app.example")…`).
    /// It wraps the whole route tree **outermost**, so a CORS preflight (`OPTIONS`)
    /// is answered before any guard or interceptor runs.
    pub fn cors(mut self, cors: Cors) -> Self {
        self.cors = Some(cors);
        self
    }

    /// Terminate TLS at the transport: serve HTTPS directly from the given
    /// [`TlsConfig`] (poem's `rustls` listener) instead of plain HTTP. Without
    /// this call the transport stays plaintext — the dev default. Pair it with
    /// [`TlsConfig::from_env`] so `main` opts in only when the certs are
    /// injected:
    ///
    /// ```no_run
    /// # use nestrs_http::{HttpTransport, TlsConfig};
    /// let mut http = HttpTransport::new();
    /// if let Some(tls) = TlsConfig::from_env()? {
    ///     http = http.tls(tls);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Mount an extra endpoint at `path`. The builder closure runs at
    /// [`Transport::configure`] time with the live container, so it can
    /// resolve services to construct framework-specific endpoints (a
    /// GraphQL schema built from container-resolved resolvers, an MCP
    /// streamable HTTP service, …).
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

    /// Take the endpoint assembled by [`Transport::configure`] for in-process
    /// testing — drive it with `poem`'s `TestClient` (via
    /// [`nestrs-testing`](https://docs.rs/nestrs-testing)) instead of binding a
    /// socket. Returns `None` before `configure` has run, and leaves the
    /// transport without an endpoint (so it must not also be `serve`d). The
    /// extracted endpoint carries the full discovery + interceptor / guard /
    /// filter chain, so a test exercises exactly what production serves.
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
        // CORS wraps outermost, so a preflight is handled before guards run.
        if let Some(cors) = self.cors.take() {
            endpoint = endpoint.with(cors).map_to_response().boxed();
        }
        // A fresh request scope is installed before anything else runs, so guards
        // and handlers can resolve request-scoped providers via `Scoped<T>`.
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
        // `.boxed()` unifies the plain and rustls listeners to one `BoxListener`,
        // so the server-run is written once whichever path is taken.
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
