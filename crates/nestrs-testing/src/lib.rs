//! In-process testing harness for nestrs.
//!
//! [`TestApp`] boots an app's real dependency-injection graph — the same
//! four-phase [`AppBuilder`](nestrs_core::AppBuilder) build production uses, with
//! the access-graph contract enforced — and exposes its HTTP surface through
//! `poem`'s `TestClient` without binding a socket. Because GraphQL, OpenAPI and
//! MCP all self-mount as HTTP endpoints, a single client exercises every surface,
//! so wiring that previously only surfaced under `curl` against a running binary
//! is now reachable from `cargo test`.
//!
//! Swap a real provider for a fake with [`TestAppBuilder::override_dyn`] /
//! [`override_value`](TestAppBuilder::override_value) — the NestJS
//! `overrideProvider` analog.
//!
//! ```ignore
//! use nestrs_testing::TestApp;
//!
//! let app = TestApp::for_module::<AppModule>().await?;
//! let resp = app.http().get("/users").send().await;
//! resp.assert_status_is_ok();
//!
//! // With a mock swapped in:
//! let app = TestApp::builder()
//!     .module::<AppModule>()
//!     .override_dyn::<dyn Clock>(std::sync::Arc::new(FrozenClock))
//!     .build()
//!     .await?;
//! ```

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use nestrs_core::{App, AppBuilder, Container, Module, Transport};
use nestrs_http::HttpTransport;
use poem::endpoint::BoxEndpoint;
use poem::Response;

/// `poem`'s test client and its assertion helpers, re-exported so a test crate
/// needs no direct `poem` dependency — mirroring how each surface wraps its
/// backing crate.
pub use poem::test::{TestClient, TestForm, TestJson, TestRequestBuilder, TestResponse};

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
}
