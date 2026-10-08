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

/// The transport the app's own `HttpModule::for_root(cfg)` would run; `None`
/// when the app imports no `HttpModule`.
fn http_from_config(container: &Container) -> Result<Option<HttpTransport>> {
    match container.get::<HttpConfig>() {
        Some(cfg) => Ok(Some(HttpTransport::from_config(&cfg)?)),
        None => Ok(None),
    }
}

type TestEndpoint = BoxEndpoint<'static, Response>;

/// A booted app plus a `poem` [`TestClient`] over its mounted endpoint, driving
/// REST, GraphQL, OpenAPI and MCP through [`http`](Self::http) without a socket.
pub struct TestApp {
    app: App,
    client: TestClient<TestEndpoint>,
}

impl TestApp {
    /// Start a [`TestAppBuilder`] to register modules and override providers.
    pub fn builder() -> TestAppBuilder {
        TestAppBuilder::new()
    }

    /// Boot a single root module with defaults.
    pub async fn for_module<M: Module + 'static>() -> Result<TestApp> {
        TestAppBuilder::new().module::<M>().build().await
    }

    /// The HTTP test client for issuing requests against the mounted routes.
    pub fn http(&self) -> &TestClient<TestEndpoint> {
        &self.client
    }

    /// Open a graphql-ws connection against the app's composed schema, for the
    /// subscriptions the HTTP client cannot speak; see [`crate::graphql`].
    #[cfg(feature = "graphql")]
    pub fn graphql_socket(&self) -> crate::graphql::GraphqlSocketBuilder {
        crate::graphql::GraphqlSocketBuilder::new(self.container().clone())
    }

    /// The DI [`Container`], for resolving providers directly in assertions.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// Re-runs the init phases (`OnModuleInit` / `OnApplicationBootstrap`),
    /// which [`TestAppBuilder::build`] already ran once.
    pub async fn init(&self) -> Result<()> {
        self.app.init().await
    }
}

/// Builder for a [`TestApp`]: [`AppBuilder`]'s registration surface plus
/// test-only overrides. Defaults `<PREFIX>_ENV=test` and loads the project `.env`
/// cascade.
pub struct TestAppBuilder {
    inner: AppBuilder,
    http: Option<HttpTransport>,
}

impl TestAppBuilder {
    fn new() -> Self {
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

    /// Replace a concrete provider with a test double by value; never the
    /// database.
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

    /// Replace a concrete provider with a pre-shared `Arc<T>`, so the test can
    /// keep inspecting the fake.
    pub fn override_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.override_arc(value);
        self
    }

    /// Supply an [`HttpTransport`] instead of the one the app's own
    /// `HttpModule::for_root(cfg)` contributes.
    ///
    /// Only for a transport the app does not declare: to test a non-default
    /// `HttpConfig`, pin it on the module.
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

    /// Build the app, configure the HTTP transport and run the init phases.
    pub async fn build(self) -> Result<TestApp> {
        let app = self.inner.build().await?;
        // A default transport would ignore every field the module configured,
        // so the harness boots the one `App::run` resolves.
        let mut transport = match self.http {
            Some(explicit) => explicit,
            None => http_from_config(app.container())?.unwrap_or_default(),
        };
        transport.configure(app.container()).await?;
        app.init().await?;
        let endpoint = transport
            .take_endpoint()
            .expect("HttpTransport::configure populates the endpoint");
        Ok(TestApp {
            app,
            client: TestClient::new(endpoint),
        })
    }

    /// Boot the app on a **real** local port and hand back a socket driver,
    /// for the WS upgrade `TestClient` cannot reach.
    #[cfg(feature = "ws")]
    pub async fn build_ws(self) -> Result<crate::ws::WsApp> {
        let app = self.inner.build().await?;
        let transport = match self.http {
            Some(explicit) => explicit,
            None => http_from_config(app.container())?.unwrap_or_default(),
        };
        crate::ws::serve(HeadlessApp::new(app), transport).await
    }

    /// Boot without an HTTP surface, for queue workers and schedulers; drive
    /// their transports through [`HeadlessApp::spawn_transport`].
    pub async fn build_headless(self) -> Result<HeadlessApp> {
        let app = self.inner.build().await?;
        Ok(HeadlessApp::new(app))
    }
}

#[cfg(feature = "opentelemetry")]
impl TestAppBuilder {
    /// Install console-only test OpenTelemetry once, which `OpenTelemetryModule`'s
    /// boot guard requires.
    pub fn with_test_telemetry(self) -> Self {
        nest_rs_opentelemetry::OpenTelemetry::init_for_tests();
        self
    }
}
