//! Runs one apalis worker per discovered `#[processor]` on a shared [`Monitor`].

use anyhow::{Context, Result};
use apalis::prelude::Monitor;
use async_trait::async_trait;
use nestrs_core::{Container, DiscoveryService, Transport};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::connection::QueueConnection;
use crate::processor::ProcessorMeta;

pub struct QueueWorker {
    processors: Vec<Arc<ProcessorMeta>>,
    container: Option<Container>,
}

impl QueueWorker {
    pub fn new() -> Self {
        Self {
            processors: Vec::new(),
            container: None,
        }
    }
}

impl Default for QueueWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for QueueWorker {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        let discovery = DiscoveryService::new(container);
        self.processors = discovery
            .meta::<ProcessorMeta>()
            .into_iter()
            .map(|d| d.meta)
            .collect();

        // Fail fast at boot if processors exist but no connection is seeded.
        if !self.processors.is_empty() {
            container.get::<QueueConnection>().context(
                "QueueWorker found #[processor]s but no QueueConnection in the container — \
                 seed one with App::builder().provide_factory(|_| QueueConnection::connect(url))",
            )?;
            for p in &self.processors {
                tracing::info!(
                    target: "nestrs::queue",
                    processor = p.name,
                    queue = p.queue,
                    concurrency = p.concurrency,
                    retries = p.retries,
                    "registered queue processor",
                );
            }
        }

        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        // No processors: idle until shutdown so this transport doesn't race
        // the app down when it is the only one attached.
        if self.processors.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        let container = self
            .container
            .expect("QueueWorker::configure must run before serve");
        let connection = container
            .get::<QueueConnection>()
            .expect("QueueConnection presence is verified in configure");

        let mut monitor = Monitor::new();
        for meta in &self.processors {
            monitor = (meta.register)(monitor, (*connection).clone(), container.clone(), meta);
        }

        monitor
            .run_with_signal(async move {
                cancel.cancelled().await;
                Ok(())
            })
            .await?;
        Ok(())
    }
}
