//! [`TestApp`] and its [`TestAppBuilder`] — boot an app's real DI graph in-process
//! and drive its HTTP surface through `poem`'s `TestClient`.

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use nestrs_core::{App, AppBuilder, Container, Module, Transport};
use nestrs_http::HttpTransport;
use poem::endpoint::BoxEndpoint;
use poem::test::TestClient;
use poem::Response;

use crate::headless::HeadlessApp;

/// The boxed, fully-assembled HTTP endpoint a [`TestApp`] drives.
type TestEndpoint = BoxEndpoint<'static, Response>;

/// A booted app under test: its assembled [`Container`] plus a [`TestClient`]
/// over the configured HTTP endpoint.
pub struct TestApp {
    app: App,
    client: TestClient<TestEndpoint>,
}

impl TestApp {
    /// Start a [`TestAppBuilder`].
    pub fn builder() -> TestAppBuilder {
        TestAppBuilder::new()
    }

    /// Boot a root module with the default [`HttpTransport`] and no overrides —
    /// the common case.
    pub async fn for_module<M: Module + 'static>() -> Result<TestApp> {
        TestAppBuilder::new().module::<M>().build().await
    }

    /// The `poem` test client over the configured HTTP surface. Fire requests
    /// with `.get(path)`, `.post(path)`, … then `.send().await`.
    pub fn http(&self) -> &TestClient<TestEndpoint> {
        &self.client
    }

    /// The assembled container, to resolve providers and assert their state.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// Run the init lifecycle hooks (`OnModuleInit`, then
    /// `OnApplicationBootstrap`) — the NestJS `app.init()` analog. Deliberately
    /// **not** run by [`build`](TestAppBuilder::build), matching NestJS's
    /// `Test...compile()`, so a test that wants startup side effects opts in.
    pub async fn init(&self) -> Result<()> {
        self.app.init().await
    }
}

/// Builder for a [`TestApp`]: declare the module tree, seed runtime values,
/// override providers with fakes, optionally supply a pre-configured
/// [`HttpTransport`], then [`build`](Self::build).
pub struct TestAppBuilder {
    inner: AppBuilder,
    http: Option<HttpTransport>,
}

impl TestAppBuilder {
    fn new() -> Self {
        // Tests run in the `Test` environment: `.env.local` is skipped (hermetic)
        // and env-aware defaults (the GraphQL playground / SDL emit, …) stay off,
        // so an e2e never writes a stray dev artifact. Set before the build reads
        // `NESTRS_ENV`; an explicit value (e.g. CI asserting prod behaviour) wins.
        if std::env::var_os("NESTRS_ENV").is_none() {
            std::env::set_var("NESTRS_ENV", "test");
        }
        Self {
            inner: App::builder(),
            http: None,
        }
    }

    /// Add a root module (delegates to [`AppBuilder::module`]).
    pub fn module<M: Module + 'static>(mut self) -> Self {
        self.inner = self.inner.module::<M>();
        self
    }

    /// Seed a runtime value (delegates to [`AppBuilder::provide`]).
    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.inner = self.inner.provide(value);
        self
    }

    /// Seed a shared `Arc<T>` (delegates to [`AppBuilder::provide_arc`]).
    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.provide_arc(value);
        self
    }

    /// Seed a `dyn Trait` binding (delegates to [`AppBuilder::provide_dyn`]).
    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.provide_dyn(value);
        self
    }

    /// Register an async factory (delegates to [`AppBuilder::provide_factory`]) —
    /// e.g. a test database pool built before the module tree wires.
    pub fn provide_factory<T, F, Fut>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.inner = self.inner.provide_factory(factory);
        self
    }

    /// Replace a concrete provider with a fake (delegates to
    /// [`AppBuilder::override_value`]). Reaches consumers resolved from the final
    /// container; see that method for the eager-build caveat.
    pub fn override_value<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.inner = self.inner.override_value(value);
        self
    }

    /// Replace a `dyn Trait` binding with a fake (delegates to
    /// [`AppBuilder::override_dyn`]) — the usual way to mock a service injected
    /// behind a trait.
    pub fn override_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.override_dyn(value);
        self
    }

    /// Use a pre-configured [`HttpTransport`] (global guards / interceptors /
    /// filters) instead of the default, so the test mirrors `main`.
    pub fn http(mut self, transport: HttpTransport) -> Self {
        self.http = Some(transport);
        self
    }

    /// Run the four-phase build (access-graph check included), configure the
    /// HTTP transport in-process against the assembled container, and return the
    /// [`TestApp`]. Propagates a factory error or an access-graph violation.
    pub async fn build(self) -> Result<TestApp> {
        let app = self.inner.build().await?;
        let mut transport = self.http.unwrap_or_default();
        transport.configure(app.container()).await?;
        let endpoint = transport
            .take_endpoint()
            .expect("HttpTransport::configure populates the endpoint");
        Ok(TestApp {
            app,
            client: TestClient::new(endpoint),
        })
    }

    /// Run the four-phase build and return the booted app **without** an HTTP
    /// surface — for an app whose transports are not HTTP (a queue worker, a
    /// scheduler). The DI graph, factory phase and access-graph check run exactly
    /// as in production, so booting alone already exercises an app's wiring. Drive
    /// its transports for a bounded window with
    /// [`HeadlessApp::spawn_transport`].
    pub async fn build_headless(self) -> Result<HeadlessApp> {
        let app = self.inner.build().await?;
        Ok(HeadlessApp::new(app))
    }
}

#[cfg(feature = "telemetry")]
impl TestAppBuilder {
    /// Satisfy the `TelemetryModule` boot guard for an app that imports it: it
    /// panics at boot unless [`Telemetry::init`](nestrs_telemetry::Telemetry::init)
    /// has run, so this installs console-only test telemetry once (idempotent).
    /// Enable the `telemetry` feature. Without it, such an app's e2e would have to
    /// hand-roll the same one-shot init.
    pub fn with_test_telemetry(self) -> Self {
        nestrs_telemetry::Telemetry::init_for_tests();
        self
    }
}
