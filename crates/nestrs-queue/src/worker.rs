//! Runs one apalis worker per discovered `#[processor]` on a shared [`Monitor`].

use anyhow::{Context, Result};
use apalis::prelude::Monitor;
use async_trait::async_trait;
use nestrs_core::{inventory, Container, DiscoveryService, ReachableProviders, Transport};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::connection::QueueConnection;
use crate::processor::{ProcessMethod, ProcessorMeta};

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
        let mut processors: Vec<Arc<ProcessorMeta>> = discovery
            .meta::<ProcessorMeta>()
            .into_iter()
            .map(|d| d.meta)
            .collect();
        // Drain link-time `#[process]` methods, filtered by ReachableProviders
        // so a method on a provider not in the app's module tree compiles in
        // but does not subscribe to its queue.
        let reachable = container.get::<ReachableProviders>();
        for entry in inventory::iter::<ProcessMethod>() {
            let provider_id = (entry.provider_type_id)();
            if let Some(r) = reachable.as_ref() {
                if !r.0.contains(&provider_id) {
                    tracing::debug!(
                        target: "nestrs::queue",
                        processor = entry.name,
                        queue = entry.queue,
                        "skipped #[process] method: provider unreachable from app's module tree",
                    );
                    continue;
                }
            }
            processors.push(Arc::new(ProcessorMeta {
                name: entry.name,
                queue: entry.queue,
                concurrency: entry.concurrency,
                retries: entry.retries,
                register: entry.register,
            }));
        }
        self.processors = processors;

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
