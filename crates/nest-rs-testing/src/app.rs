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
/// test-only overrides. Defaults `<PREFIX>_ENV=test`, loads the project `.env`
/// cascade, and publishes the tested package's name, version and description as
/// the process's [`AppIdentity`](nest_rs_core::AppIdentity), as
/// `#[nest_rs::main]` publishes the binary's.
///
/// The boot arms the outbox of an app binding a store and a queue backend
/// (feature `queue`), as the binary's relay does when served, without running
/// the relay: a push inside a unit waits in the store, and a test needing its
/// job run spawns `OutboxRelay` beside its worker.
pub struct TestAppBuilder {
    inner: AppBuilder,
    /// The database [`ephemeral_database`](Self::ephemeral_database) creates
    /// at the boot.
    #[cfg(feature = "orm")]
    pub(crate) database: Option<crate::database::DatabaseFixture>,
}

/// What a booted test app keeps alive beside its container, dropped after it.
#[derive(Default)]
pub(crate) struct Kept {
    #[cfg(feature = "orm")]
    #[expect(
        dead_code,
        reason = "held for its drop, which drops the database once the app is gone"
    )]
    pub(crate) database: Option<crate::EphemeralDatabase>,
}

impl TestAppBuilder {
    pub(crate) fn new() -> Self {
        crate::env::load_project_env();
        crate::package::publish_package_identity();
        Self {
            inner: App::builder(),
            #[cfg(feature = "orm")]
            database: None,
        }
    }

    /// Build the app: its database first, the connection seeded, then the
    /// four phases, then its outbox armed. A boot failing past the build is
    /// closed before the error returns.
    pub(crate) async fn boot(self) -> Result<(App, Kept)> {
        #[cfg(feature = "orm")]
        let (inner, kept) = crate::database::seeded(self.inner, self.database).await?;
        #[cfg(not(feature = "orm"))]
        let (inner, kept) = (self.inner, Kept::default());
        let app = inner.build().await?;
        #[cfg(feature = "queue")]
        if let Err(refused) = nest_rs_queue::__private::arm_outbox(app.container()).await {
            app.close().await;
            return Err(refused);
        }
        Ok((app, kept))
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
        let (app, kept) = self.boot().await?;
        Ok(HeadlessApp::new(app, kept))
    }
}

/// The HTTP half (feature `http`): the app booted with its HTTP transport
/// mounted, and the builder's seams that only an HTTP app reads.
#[cfg(feature = "http")]
mod http {
    use std::sync::Arc;

    use anyhow::Result;
    use nest_rs_core::{App, AppLifecycle, Container, Module, Transport};
    use nest_rs_http::{HttpConfig, HttpTransport};

    use super::{Kept, TestAppBuilder};
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
        /// Dropped after the app, which may still hold its connection.
        kept: Kept,
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
            // The database the app ran on is dropped last, at the end of scope.
            let TestApp {
                app,
                client,
                kept: _kept,
            } = self;
            // The mounted endpoint holds the container too.
            drop(client);
            app.close().await;
        }
    }

    #[cfg(feature = "authn")]
    impl TestApp {
        /// The `Authorization` value of a bearer token the app's own
        /// `JwtService` signs over the claims `claims` builds from the expiry
        /// the service stamps: a token the app's authentication guard admits,
        /// signed with the key it verifies with. Panics when the app provides
        /// no `JwtService` or one holding no signing key.
        ///
        /// ```
        /// # use nest_rs_authn::{AuthnConfig, AuthnModule, JwtService};
        /// # use nest_rs_core::{Secret, module};
        /// # use nest_rs_testing::TestApp;
        /// # use serde::Serialize;
        /// #[derive(Serialize)]
        /// struct Claims {
        ///     sub: String,
        ///     exp: u64,
        /// }
        ///
        /// #[module(imports = [AuthnModule])]
        /// struct BlogModule;
        /// # #[nest_rs_core::main]
        /// # async fn main() -> anyhow::Result<()> {
        ///
        /// let app = TestApp::builder()
        ///     .module::<BlogModule>()
        /// #   .provide(AuthnConfig {
        /// #       secret: Some(Secret::new("a-test-secret-of-thirty-two-bytes!".into())),
        /// #       ..AuthnConfig::default()
        /// #   })
        ///     .build()
        ///     .await?;
        /// let authorization = app.bearer(|exp| Claims { sub: "ada".into(), exp });
        /// # let jwt = app.container().get::<JwtService>().expect("provided");
        /// # let token = authorization.strip_prefix("Bearer ").expect("a bearer value");
        /// # let read: serde_json::Value = jwt.verify(token).await?;
        /// # assert_eq!(read["sub"], "ada");
        /// # Ok(())
        /// # }
        /// ```
        pub fn bearer<C: serde::Serialize>(&self, claims: impl FnOnce(u64) -> C) -> String {
            let jwt = self
                .container()
                .get::<nest_rs_authn::JwtService>()
                .expect("the app provides no JwtService: import the module authenticating it");
            let token = jwt
                .sign(&claims(jwt.expiry()))
                .unwrap_or_else(|refused| panic!("the app's JwtService signs no token: {refused}"));
            format!("Bearer {token}")
        }
    }

    #[cfg(feature = "authz")]
    impl TestApp {
        /// Run `body` in one unit of work of the app's bound `UnitOfWork`, as a
        /// request granted `ability`: the service it calls reads the ambient
        /// executor and ability an edge installs, and the unit commits on `Ok`
        /// and rolls back on `Err`.
        ///
        /// # Panics
        ///
        /// When the app binds no unit of work, and when the binding cannot
        /// honour the unit — it refuses to open it, its commit fails, or the
        /// unit's outcome is in doubt.
        pub async fn unit<T, E, F, Fut>(
            &self,
            ability: std::sync::Arc<nest_rs_authz::Ability>,
            body: F,
        ) -> Result<T, E>
        where
            T: Send,
            E: Send,
            F: FnOnce() -> Fut,
            Fut: std::future::Future<Output = Result<T, E>> + Send,
        {
            crate::unit::run(
                self.container(),
                nest_rs_database::ExecutorScope::Request,
                nest_rs_authz::__private::install_ability(ability, body()),
            )
            .await
        }
    }

    #[cfg(feature = "database")]
    impl TestApp {
        /// Run `body` in one unit of work of the app's bound `UnitOfWork`, as
        /// system work — a job, a tick — which no ability scopes: it commits on
        /// `Ok` and rolls back on `Err`.
        ///
        /// # Panics
        ///
        /// As [`unit`](Self::unit) does: when the app binds no unit of work, and
        /// when the binding cannot honour the unit.
        pub async fn system_unit<T, E, F, Fut>(&self, body: F) -> Result<T, E>
        where
            T: Send,
            E: Send,
            F: FnOnce() -> Fut,
            Fut: std::future::Future<Output = Result<T, E>> + Send,
        {
            crate::unit::run(
                self.container(),
                nest_rs_database::ExecutorScope::Job,
                body(),
            )
            .await
        }
    }

    impl TestAppBuilder {
        /// Build the app, configure the HTTP transport and run the init phases —
        /// once: they do not run again for this app. A boot that fails past the
        /// build is closed as [`App::run`] closes it before the error returns.
        pub async fn build(self) -> Result<TestApp> {
            let (app, kept) = self.boot().await?;
            match mount(&app).await {
                Ok(client) => Ok(TestApp { app, client, kept }),
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
            let (app, kept) = self.boot().await?;
            let config = http_config(app.container());
            crate::ws::serve(crate::HeadlessApp::new(app, kept), &config).await
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
