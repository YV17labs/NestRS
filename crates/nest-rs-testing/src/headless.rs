//! Booted app with no HTTP client — for queue workers, schedulers, etc.

use anyhow::Result;
use nest_rs_core::{App, Container, Transport};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Runs non-HTTP [`Transport`]s on a cancellable background task so a test
/// can enqueue, observe, then shut the transport down.
pub struct HeadlessApp {
    app: App,
}

impl HeadlessApp {
    pub(crate) fn new(app: App) -> Self {
        Self { app }
    }

    /// The DI [`Container`], for resolving providers directly in assertions.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// Run the app's init phases (lifecycle hooks, bootstrap wiring) without
    /// standing up any transport.
    pub async fn init(&self) -> Result<()> {
        self.app.init().await
    }

    /// Configure and run a non-HTTP [`Transport`] on a cancellable background
    /// task, returning a [`TransportHandle`] to observe then shut it down.
    pub async fn spawn_transport<T: Transport>(&self, mut transport: T) -> Result<TransportHandle> {
        transport.configure(self.container()).await?;
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let join = tokio::spawn(async move { Box::new(transport).serve(token).await });
        Ok(TransportHandle { cancel, join })
    }

    /// Unwrap the underlying [`App`], for assertions that need the app by value.
    pub fn into_app(self) -> App {
        self.app
    }
}

/// Dropping detaches the task; [`shutdown`](Self::shutdown) cancels and awaits.
pub struct TransportHandle {
    cancel: CancellationToken,
    join: JoinHandle<Result<()>>,
}

impl TransportHandle {
    /// Cancel the transport task and await its clean exit, surfacing any error
    /// it terminated with.
    pub async fn shutdown(self) -> Result<()> {
        self.cancel.cancel();
        self.join.await.map_err(|e| anyhow::anyhow!(e))?
    }
}
