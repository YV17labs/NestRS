//! Boot an app's real DI graph in-process and drive its HTTP surface through
//! `poem`'s `TestClient`.

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use nest_rs_core::{App, AppBuilder, Container, Module, Transport};
use nest_rs_exception_filters::{AppBuilderExceptionFiltersExt, ExceptionFilterSpec};
use nest_rs_filters::{AppBuilderFiltersExt, FilterSpec};
use nest_rs_guards::{AppBuilderGuardsExt, AppBuilderPipesExt, GuardSpec, PipeSpec};
use nest_rs_http::{HttpConfig, HttpTransport};
use nest_rs_interceptors::{AppBuilderInterceptorsExt, InterceptorSpec};
use poem::Response;
use poem::endpoint::BoxEndpoint;
use poem::test::TestClient;

use crate::headless::HeadlessApp;

/// The transport the app's own `HttpModule::for_root(cfg)` would run, built from
/// the very `HttpConfig` the container resolved.
///
/// `Ok(None)` means the app imports no `HttpModule`, which is legitimate — a
/// suite may drive controllers through a bare transport — and only then does the
/// harness fall back to a default.
fn http_from_config(container: &Container) -> Result<Option<HttpTransport>> {
    match container.get::<HttpConfig>() {
        Some(cfg) => Ok(Some(HttpTransport::from_config(&cfg)?)),
        None => Ok(None),
    }
}

type TestEndpoint = BoxEndpoint<'static, Response>;

/// A booted app plus a `poem` [`TestClient`] over its mounted endpoint — the
/// default e2e entry point. Drive every HTTP-borne surface (REST, GraphQL,
/// OpenAPI, MCP) through [`http`](Self::http) without binding a socket.
pub struct TestApp {
    app: App,
    client: TestClient<TestEndpoint>,
}

impl TestApp {
    /// Start a [`TestAppBuilder`] to register modules and override providers.
    pub fn builder() -> TestAppBuilder {
        TestAppBuilder::new()
    }

    /// Shorthand for booting a single root module with defaults — no overrides,
    /// no extra transport config.
    pub async fn for_module<M: Module + 'static>() -> Result<TestApp> {
        TestAppBuilder::new().module::<M>().build().await
    }

    /// The HTTP test client for issuing requests against the mounted routes.
    pub fn http(&self) -> &TestClient<TestEndpoint> {
        &self.client
    }

    /// Open a graphql-ws connection against the app's composed schema — the
    /// protocol a subscription rides, which the HTTP client cannot speak. See
    /// [`crate::graphql`] for what it does and does not exercise.
    #[cfg(feature = "graphql")]
    pub fn graphql_socket(&self) -> crate::GraphqlSocketBuilder {
        crate::GraphqlSocketBuilder::new(self.container().clone())
    }

    /// The DI [`Container`], for resolving providers directly in assertions.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// Re-runs the application's startup init phases (`OnModuleInit` /
    /// `OnApplicationBootstrap`). [`TestAppBuilder::build`] already runs them
    /// once, so a `TestApp` is fully started on return — call this only to drive
    /// the phases again after a mid-test change that must re-trigger bootstrap
    /// wiring. (`HeadlessApp`, built without HTTP, does **not** auto-init — there
    /// it is the required startup call.)
    pub async fn init(&self) -> Result<()> {
        self.app.init().await
    }
}

/// Builder for a [`TestApp`]: mirrors [`AppBuilder`]'s registration surface and
/// adds test-only provider overrides. Defaults `NESTRS_ENV=test` (hermetic) and
/// loads the project `.env` cascade so e2e picks up the devcontainer hostnames.
pub struct TestAppBuilder {
    inner: AppBuilder,
    http: Option<HttpTransport>,
}

impl TestAppBuilder {
    fn new() -> Self {
        // Every e2e boot (any transport) sees the project's own `.env`.
        // `load_project_env` also defaults `NESTRS_ENV=test` (set-if-absent)
        // *inside* its `Once`, so the invariant holds whichever entry point
        // ran first — see `env.rs`.
        crate::env::load_project_env();
        Self {
            inner: App::builder(),
            http: None,
        }
    }

    /// Register a root module, exactly as [`AppBuilder::module`].
    pub fn module<M: Module + 'static>(mut self) -> Self {
        self.inner = self.inner.module::<M>();
        self
    }

    /// Seed a runtime value as a singleton provider (the `main`-supplied seed
    /// path), exactly as [`AppBuilder::provide`].
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.inner = self.inner.provide(value);
        self
    }

    /// Seed a pre-shared `Arc<T>` as a singleton, exactly as
    /// [`AppBuilder::provide_arc`].
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.provide_arc(value);
        self
    }

    /// Seed a trait object under its `dyn Trait` type, exactly as
    /// [`AppBuilder::provide_dyn`].
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.provide_dyn(value);
        self
    }

    /// Register an async factory whose output is injectable, exactly as
    /// [`AppBuilder::provide_factory`].
    pub fn provide_factory<T, F, Fut>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.inner = self.inner.provide_factory(factory);
        self
    }

    /// Replace a concrete provider with a test double by value — the standard
    /// way to swap a real dependency for a fake before boot. Never use it to
    /// mock the database (a hard no for e2e).
    pub fn override_value<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.inner = self.inner.override_value(value);
        self
    }

    /// Replace a `dyn Trait` provider with a test double behind an `Arc`, for
    /// impls consumers inject as `Arc<dyn Trait>`.
    pub fn override_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.override_dyn(value);
        self
    }

    /// Replace a concrete provider with a pre-shared `Arc<T>`. Use when a test
    /// already holds the fake in an `Arc` — typically because it inspects the
    /// fake's state after a request — and would otherwise have to give up
    /// ownership through [`override_value`](Self::override_value).
    pub fn override_provider<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.override_provider(value);
        self
    }

    /// Supply an [`HttpTransport`] instead of the one the app's own
    /// `HttpModule::for_root(cfg)` contributes.
    ///
    /// Reach for it only when the test needs a transport the app does **not**
    /// declare. To assert against a non-default `HttpConfig`, pin it on the
    /// module — the harness now boots what that config describes, so the test
    /// and the deployment cannot diverge.
    pub fn http(mut self, transport: HttpTransport) -> Self {
        self.http = Some(transport);
        self
    }

    /// Forwards to [`AppBuilderGuardsExt::use_guards_global`].
    pub fn use_guards_global<I>(mut self, specs: I) -> Self
    where
        I: IntoIterator<Item = GuardSpec>,
    {
        self.inner = self.inner.use_guards_global(specs);
        self
    }

    /// Forwards to [`AppBuilderPipesExt::use_pipes_global`].
    pub fn use_pipes_global<I>(mut self, specs: I) -> Self
    where
        I: IntoIterator<Item = PipeSpec>,
    {
        self.inner = self.inner.use_pipes_global(specs);
        self
    }

    /// Forwards to [`AppBuilderInterceptorsExt::use_interceptors_global`].
    pub fn use_interceptors_global<I>(mut self, specs: I) -> Self
    where
        I: IntoIterator<Item = InterceptorSpec>,
    {
        self.inner = self.inner.use_interceptors_global(specs);
        self
    }

    /// Forwards to [`AppBuilderFiltersExt::use_filters_global`].
    pub fn use_filters_global<I>(mut self, specs: I) -> Self
    where
        I: IntoIterator<Item = FilterSpec>,
    {
        self.inner = self.inner.use_filters_global(specs);
        self
    }

    /// Forwards to
    /// [`AppBuilderExceptionFiltersExt::use_exception_filters_global`].
    pub fn use_exception_filters_global<I>(mut self, specs: I) -> Self
    where
        I: IntoIterator<Item = ExceptionFilterSpec>,
    {
        self.inner = self.inner.use_exception_filters_global(specs);
        self
    }

    /// Run the four-phase build, configure the HTTP transport, run the init
    /// phases (so bootstrap-time wiring like health indicators and the social
    /// registry populate), and hand back a ready [`TestApp`].
    pub async fn build(self) -> Result<TestApp> {
        let app = self.inner.build().await?;
        // The transport the app's own `HttpModule::for_root(cfg)` contributed,
        // exactly as `App::run` resolves it — not a fresh `HttpTransport::new()`.
        //
        // A default one ignores every field the module configured: the global
        // prefix, the versioning strategy, the body cap, the request timeout,
        // CORS, compression, the security headers. A suite built on it asserted
        // against a transport the deployment never runs, which is the failure
        // mode e2e exists to catch — so the harness has to boot what ships.
        let mut transport = match self.http {
            Some(explicit) => explicit,
            None => http_from_config(app.container())?.unwrap_or_default(),
        };
        transport.configure(app.container()).await?;
        // Drive the same startup the server performs (`App::run` configures the
        // transport, then runs the init phases). Without this, modules whose
        // wiring lands in `OnApplicationBootstrap` — health indicators, the
        // social provider registry — stay unpopulated under the test harness.
        app.init().await?;
        let endpoint = transport
            .take_endpoint()
            .expect("HttpTransport::configure populates the endpoint");
        Ok(TestApp {
            app,
            client: TestClient::new(endpoint),
        })
    }

    /// Boot the app on a **real** local port and hand back a socket driver.
    ///
    /// The counterpart to [`build`](Self::build) for the one edge `TestClient`
    /// cannot reach: a WS upgrade produces a connection task, and everything
    /// the gateway does above `dispatch` — the resolved `WsConfig`, the
    /// lifetime ceiling, the writer task, the per-message request scope — only
    /// exists inside it. Same transport either way: the one the app's own
    /// `HttpModule::for_root(cfg)` describes, so the global prefix and the rest
    /// match what ships.
    #[cfg(feature = "ws")]
    pub async fn build_ws(self) -> Result<crate::ws::WsApp> {
        let app = self.inner.build().await?;
        let transport = match self.http {
            Some(explicit) => explicit,
            None => http_from_config(app.container())?.unwrap_or_default(),
        };
        crate::ws::serve(HeadlessApp::new(app), transport).await
    }

    /// Boot without an HTTP surface, for queue workers / schedulers. The
    /// four-phase build (including the access-graph check) still runs. Drive
    /// non-HTTP transports through [`HeadlessApp::spawn_transport`].
    pub async fn build_headless(self) -> Result<HeadlessApp> {
        let app = self.inner.build().await?;
        Ok(HeadlessApp::new(app))
    }
}

#[cfg(feature = "opentelemetry")]
impl TestAppBuilder {
    /// Satisfy `OpenTelemetryModule`'s boot guard (it panics unless `OpenTelemetry::init` has
    /// run) by installing console-only test OpenTelemetry once (idempotent).
    pub fn with_test_telemetry(self) -> Self {
        nest_rs_opentelemetry::OpenTelemetry::init_for_tests();
        self
    }
}
