//! Booted app with no HTTP client — for queue workers, schedulers, etc.

use anyhow::Result;
use nestrs_core::{App, Container, Transport};
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

    pub fn container(&self) -> &Container {
        self.app.container()
    }

    pub async fn init(&self) -> Result<()> {
        self.app.init().await
    }

    pub async fn spawn_transport<T: Transport>(&self, mut transport: T) -> Result<TransportHandle> {
        transport.configure(self.container()).await?;
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let join = tokio::spawn(async move { Box::new(transport).serve(token).await });
        Ok(TransportHandle { cancel, join })
    }

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
    pub async fn shutdown(self) -> Result<()> {
        self.cancel.cancel();
        self.join.await.map_err(|e| anyhow::anyhow!(e))?
    }
}
