//! [`HeadlessApp`] — a booted app with no HTTP client, for apps whose transports
//! are not HTTP (a queue worker, a scheduler). See [`TestAppBuilder::build_headless`].

use anyhow::Result;
use nestrs_core::{App, Container, Transport};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// A booted app with no HTTP client — see
/// [`TestAppBuilder::build_headless`](crate::TestAppBuilder::build_headless). It
/// exposes the assembled [`Container`] and runs non-HTTP [`Transport`]s on a
/// cancellable background task so a test can enqueue work, observe it, then shut
/// the transport down.
pub struct HeadlessApp {
    app: App,
}

impl HeadlessApp {
    pub(crate) fn new(app: App) -> Self {
        Self { app }
    }

    /// The assembled container, to resolve providers (e.g. a queue connection to
    /// enqueue against) and assert their state.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// Run the init lifecycle hooks (`OnModuleInit`, then `OnApplicationBootstrap`)
    /// — the [`TestApp::init`](crate::TestApp::init) analog for a headless app.
    pub async fn init(&self) -> Result<()> {
        self.app.init().await
    }

    /// Configure a transport against the container and start serving it on a
    /// background task, returning a [`TransportHandle`] to stop it. A `configure`
    /// failure (the regression an app's wiring most often hits — a missing
    /// discovered dependency, an unresolved connection) propagates here.
    pub async fn spawn_transport<T: Transport>(&self, mut transport: T) -> Result<TransportHandle> {
        transport.configure(self.container()).await?;
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let join = tokio::spawn(async move { Box::new(transport).serve(token).await });
        Ok(TransportHandle { cancel, join })
    }

    /// The booted [`App`], to attach transports and `run()` it directly when a
    /// test wants the full server loop rather than the bounded
    /// [`spawn_transport`](Self::spawn_transport) driver.
    pub fn into_app(self) -> App {
        self.app
    }
}

/// Handle to a transport started by [`HeadlessApp::spawn_transport`]. Dropping it
/// detaches the task; call [`shutdown`](Self::shutdown) to cancel it and await its
/// result.
pub struct TransportHandle {
    cancel: CancellationToken,
    join: JoinHandle<Result<()>>,
}

impl TransportHandle {
    /// Signal the transport to stop (the same `CancellationToken` SIGTERM trips in
    /// production) and await its `serve` future, surfacing any error it returned.
    pub async fn shutdown(self) -> Result<()> {
        self.cancel.cancel();
        self.join.await.map_err(|e| anyhow::anyhow!(e))?
    }
}
