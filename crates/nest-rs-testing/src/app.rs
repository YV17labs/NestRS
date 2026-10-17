//! Boot an app's real DI graph in-process: headless, or with its HTTP surface
//! driven through a [`TestClient`](crate::TestClient) (feature `http`).

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use nest_rs_core::{App, AppBuilder, Container, Module};

use crate::headless::HeadlessApp;

#[cfg(feature = "http")]
pub use http::TestApp;

/// Builder for a test app, from [`HeadlessApp::builder`] or, with the `http`
/// feature, `TestApp::builder`: [`AppBuilder`]'s registration surface plus
/// test-only overrides. Defaults `<PREFIX>_ENV=test` and loads the project `.env`
/// cascade.
pub struct TestAppBuilder {
    inner: AppBuilder,
}

impl TestAppBuilder {
    pub(crate) fn new() -> Self {
        crate::env::load_project_env();
        Self {
            inner: App::builder(),
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

    /// Replace the provider of `T` with a test double before anything is
    /// built from it, exactly as [`AppBuilder::override_value`]: every
    /// consumer injects the double, the provider is never built, and a type
    /// nothing in the app provides fails the boot. Never the database.
    pub fn override_value<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.inner = self.inner.override_value(value);
        self
    }

    /// Replace the trait object `Arc<dyn Trait>` with a test double, exactly
    /// as [`AppBuilder::override_dyn`] — the double of a `Foo as dyn Trait`
    /// binding, which it replaces whole.
    pub fn override_dyn<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.override_dyn(value);
        self
    }

    /// [`override_value`](Self::override_value) for a double the test
    /// already holds in an `Arc`, so it can keep inspecting it.
    pub fn override_arc<T: Any + Send + Sync>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.override_arc(value);
        self
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
        nest_rs_opentelemetry::__private::init_for_tests();
        self
    }
}

/// The HTTP half (feature `http`): the app booted with its HTTP transport
/// mounted, and the builder's seams that only an HTTP app reads.
#[cfg(feature = "http")]
mod http {
    use std::sync::Arc;

    use anyhow::Result;
    use nest_rs_core::{App, AppLifecycle, Container, Module, Transport};
    use nest_rs_exception_filters::{AppBuilderExceptionFiltersExt, ExceptionFilterSpec};
    use nest_rs_filters::{AppBuilderFiltersExt, FilterSpec};
    use nest_rs_guards::{AppBuilderGuardsExt, GuardSpec};
    use nest_rs_http::{HttpConfig, HttpTransport};
    use nest_rs_interceptors::{AppBuilderInterceptorsExt, InterceptorSpec};
    use nest_rs_pipes::{AppBuilderPipesExt, PipeSpec};

    use super::TestAppBuilder;
    use crate::TestClient;

    /// The config the app's own `HttpModule::for_root(cfg)` resolved, or the one a
    /// suite seeded; the defaults when there is neither.
    fn http_config(container: &Container) -> Arc<HttpConfig> {
        container.get::<HttpConfig>().unwrap_or_default()
    }

    /// A booted app plus a [`TestClient`] over its mounted endpoint, driving
    /// REST, GraphQL, OpenAPI and MCP through [`http`](Self::http) without a
    /// socket.
    ///
    /// [`close`](Self::close) ends it as [`App::close`] does; dropping it instead
    /// runs no shutdown hook and closes nothing.
    pub struct TestApp {
        app: App,
        client: TestClient,
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

        /// The HTTP test client for issuing requests against the mounted
        /// routes, through the stack the server runs: the edge, the router,
        /// the guards and the handlers.
        pub fn http(&self) -> &TestClient {
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

        /// The app's [`AppLifecycle`]: in-process, it stays
        /// [`Starting`](nest_rs_core::AppState::Starting), since no transport
        /// serves it, until [`close`](Self::close) turns it
        /// [`Draining`](nest_rs_core::AppState::Draining).
        pub fn lifecycle(&self) -> &Arc<AppLifecycle> {
            self.app.lifecycle()
        }

        /// Stop the app as [`App::close`] does: the shutdown phases in reverse, the
        /// resources its modules opened closed, its container dropped — within the
        /// shutdown hooks' budget.
        pub async fn close(self) {
            let TestApp { app, client } = self;
            // The mounted endpoint holds the container too.
            drop(client);
            app.close().await;
        }
    }

    impl TestAppBuilder {
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

        /// Build the app, configure the HTTP transport and run the init phases —
        /// once: they do not run again for this app. A boot that fails past the
        /// build is closed as [`App::run`] closes it before the error returns.
        pub async fn build(self) -> Result<TestApp> {
            let app = self.inner.build().await?;
            match mount(&app).await {
                Ok(client) => Ok(TestApp { app, client }),
                Err(error) => {
                    app.close().await;
                    Err(error)
                }
            }
        }

        /// Boot the app on a **real** local port and hand back a socket driver,
        /// for the WS upgrade `TestClient` cannot reach.
        #[cfg(feature = "ws")]
        pub async fn build_ws(self) -> Result<crate::ws::WsApp> {
            let app = self.inner.build().await?;
            let config = http_config(app.container());
            crate::ws::serve(crate::HeadlessApp::new(app), &config).await
        }
    }

    /// Configure the app's HTTP transport, run its init phases, and hand back a
    /// client over the endpoint it mounted.
    async fn mount(app: &App) -> Result<TestClient> {
        // A default transport would ignore every field the module configured,
        // so the harness boots the one `App::run` resolves.
        let mut transport = HttpTransport::from_config(&http_config(app.container()))?;
        transport.configure(app.container()).await?;
        // What `App::run` refuses after configuring, a route or gateway whose
        // chain did not compose, the harness refuses too.
        if let Some(refusal) = nest_rs_core::__private::take_site_refusal(app.container()) {
            return Err(refusal);
        }
        app.init().await?;
        let endpoint = transport
            .take_endpoint()
            .expect("HttpTransport::configure populates the endpoint");
        Ok(TestClient::new(endpoint))
    }
}
