//! Boot an app's real DI graph in-process and drive its HTTP surface through
//! `poem`'s `TestClient`.

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

type TestEndpoint = BoxEndpoint<'static, Response>;

pub struct TestApp {
    app: App,
    client: TestClient<TestEndpoint>,
}

impl TestApp {
    pub fn builder() -> TestAppBuilder {
        TestAppBuilder::new()
    }

    pub async fn for_module<M: Module + 'static>() -> Result<TestApp> {
        TestAppBuilder::new().module::<M>().build().await
    }

    pub fn http(&self) -> &TestClient<TestEndpoint> {
        &self.client
    }

    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// Runs the application's startup side effects: deliberately **not** run by
    /// `build`, so a test harness can compile the app without triggering them.
    pub async fn init(&self) -> Result<()> {
        self.app.init().await
    }
}

pub struct TestAppBuilder {
    inner: AppBuilder,
    http: Option<HttpTransport>,
}

impl TestAppBuilder {
    fn new() -> Self {
        // Default to NESTRS_ENV=test so `.env.local` is skipped (hermetic) and
        // env-aware defaults (GraphQL playground / SDL emit) stay off. An
        // explicit value wins (e.g. CI asserting prod behaviour).
        if std::env::var_os("NESTRS_ENV").is_none() {
            std::env::set_var("NESTRS_ENV", "test");
        }
        Self {
            inner: App::builder(),
            http: None,
        }
    }

    pub fn module<M: Module + 'static>(mut self) -> Self {
        self.inner = self.inner.module::<M>();
        self
    }

    pub fn provide<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.inner = self.inner.provide(value);
        self
    }

    pub fn provide_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.provide_arc(value);
        self
    }

    pub fn provide_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.provide_dyn(value);
        self
    }

    pub fn provide_factory<T, F, Fut>(mut self, factory: F) -> Self
    where
        T: Any + Send + Sync,
        F: FnOnce(Container) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        self.inner = self.inner.provide_factory(factory);
        self
    }

    pub fn override_value<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.inner = self.inner.override_value(value);
        self
    }

    pub fn override_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.override_dyn(value);
        self
    }

    pub fn http(mut self, transport: HttpTransport) -> Self {
        self.http = Some(transport);
        self
    }

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

    /// Boot without an HTTP surface, for queue workers / schedulers. The
    /// four-phase build (including the access-graph check) still runs. Drive
    /// non-HTTP transports through [`HeadlessApp::spawn_transport`].
    pub async fn build_headless(self) -> Result<HeadlessApp> {
        let app = self.inner.build().await?;
        Ok(HeadlessApp::new(app))
    }
}

#[cfg(feature = "telemetry")]
impl TestAppBuilder {
    /// Satisfy `TelemetryModule`'s boot guard (it panics unless
    /// `Telemetry::init` has run) by installing console-only test telemetry
    /// once (idempotent).
    pub fn with_test_telemetry(self) -> Self {
        nestrs_telemetry::Telemetry::init_for_tests();
        self
    }
}
