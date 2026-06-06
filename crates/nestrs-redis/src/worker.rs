//! apalis-redis `JobConsumer` exposed as a `Transport`: one apalis worker per
//! discovered `#[process]` method on a shared [`Monitor`].
//!
//! Every queue is consumed as `RedisStorage<serde_json::Value>` — the
//! backend-agnostic wire format — and dispatched through the type-erased
//! `JobHandler` the `#[processor]` macro emits.

use anyhow::{Context, Result};
use apalis::layers::ErrorHandlingLayer;
use apalis::layers::catch_panic::CatchPanicLayer;
use apalis::layers::retry::{RetryLayer, RetryPolicy};
use apalis::prelude::{Data, Monitor, WorkerBuilder, WorkerFactoryFn};
use async_trait::async_trait;
use nestrs_core::{Container, ReachableProviders, Transport, inventory};
use nestrs_queue::ProcessMethod;
use tokio_util::sync::CancellationToken;

use crate::connection::QueueConnection;

pub struct QueueWorker {
    methods: Vec<&'static ProcessMethod>,
    container: Option<Container>,
}

impl QueueWorker {
    pub fn new() -> Self {
        Self {
            methods: Vec::new(),
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
        // Drain link-time `#[process]` methods, filtered by ReachableProviders
        // so a method on a provider not in the app's module tree compiles in
        // but does not subscribe to its queue.
        let reachable = container.get::<ReachableProviders>();
        let mut methods: Vec<&'static ProcessMethod> = Vec::new();
        for entry in inventory::iter::<ProcessMethod>() {
            let provider_id = (entry.provider_type_id)();
            if let Some(r) = reachable.as_ref()
                && !r.0.contains(&provider_id)
            {
                tracing::debug!(
                    target: "nestrs::queue",
                    processor = entry.name,
                    queue = entry.queue,
                    "skipped #[process] method: provider unreachable from app's module tree",
                );
                continue;
            }
            methods.push(entry);
        }
        self.methods = methods;

        // Fail fast at boot if methods exist but no connection is seeded.
        if !self.methods.is_empty() {
            container.get::<QueueConnection>().context(
                "QueueWorker found #[processor]s but no QueueConnection in the container — \
                 seed one with App::builder().provide_factory(|_| QueueConnection::connect(url))",
            )?;
            for m in &self.methods {
                tracing::info!(
                    target: "nestrs::queue",
                    processor = m.name,
                    queue = m.queue,
                    concurrency = m.concurrency,
                    retries = m.retries,
                    "registered queue processor",
                );
            }
        }

        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        // No methods: idle until shutdown so this transport doesn't race
        // the app down when it is the only one attached.
        if self.methods.is_empty() {
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
        for method in &self.methods {
            monitor = build_worker(monitor, &connection, container.clone(), method);
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

/// Build one apalis worker for a `ProcessMethod`. The wire payload is always
/// `serde_json::Value`; the macro-emitted `JobHandler` deserializes it to the
/// user's `J` inside the closure, so this builder never names `J`.
fn build_worker(
    monitor: Monitor,
    conn: &QueueConnection,
    container: Container,
    method: &ProcessMethod,
) -> Monitor {
    // apalis 0.7: one worker processes its fetched batch concurrently
    // (FuturesUnordered), so `concurrency` is the Redis source's fetch buffer
    // — the ceiling on in-flight jobs — not a worker count.
    let storage = conn.consumer_storage(method.queue, method.concurrency);
    let handler = method.handler;
    // CatchPanicLayer sits *inside* the retry/error-handling layers so a
    // panic inside the user `#[process]` method (a `Container::get` cycle,
    // an `unwrap` in user code, a panicking `Deserialize` impl, …) is
    // converted into an apalis `Error::Abort` instead of propagating up
    // and aborting the worker for the whole queue. RetryLayer catches
    // `Err`, not panics, so without this layer one bad job kills the
    // queue's consumer.
    let worker = WorkerBuilder::new(method.queue)
        .layer(ErrorHandlingLayer::new())
        .layer(RetryLayer::new(RetryPolicy::retries(method.retries)))
        .layer(CatchPanicLayer::new())
        .data(container)
        .backend(storage)
        .build_fn(move |job: serde_json::Value, container: Data<Container>| {
            let container = (*container).clone();
            async move { handler(job, container).await }
        });
    monitor.register(worker)
}
