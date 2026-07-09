//! Backend + producer seams.
//!
//! Every queue backend implements [`QueueBackend`] (the boot identity used by
//! diagnostics) and exposes a [`JobProducer`] surface so any feature can
//! enqueue without naming the backend. The first-class backend is `apalis-redis`
//! (shipped as `nest-rs-queue`); third-party backends provide their own
//! `*Module` that registers a `JobProducer` in the container the same way.

use async_trait::async_trait;
use serde::Serialize;

use crate::processor::Job;

/// A queue backend, identified by name for boot diagnostics. The actual
/// runtime work happens through [`JobProducer`] (enqueue) and
/// [`JobConsumer`](crate::consumer::JobConsumer) (drain `ProcessMethod` and
/// dispatch). A backend's module typically:
///
/// 1. seeds an `Arc<dyn JobProducer>` in the container (so any service can
///    inject it generically), and
/// 2. contributes a `Transport` whose `serve` runs the backend's
///    [`JobConsumer`] driver.
pub trait QueueBackend: Send + Sync + 'static {
    /// Stable display name (e.g. `"apalis-redis"`, `"sqs"`, `"in-memory"`).
    /// Logged at boot when the consumer attaches.
    fn name(&self) -> &'static str;
}

/// Backend-agnostic producer: push a JSON-serialized job onto a named queue.
/// Concrete backends implement this; typed pushes are a convenience built on
/// top via [`JobProducerExt`].
#[async_trait]
pub trait JobProducer: Send + Sync + 'static {
    /// Push a JSON-encoded job onto `queue`. The wire format is always JSON;
    /// a backend may re-encode internally but must round-trip the payload.
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> anyhow::Result<()>;
}

/// Typed-push convenience over any [`JobProducer`]. Lives as an extension trait
/// so the producer trait stays object-safe (`Arc<dyn JobProducer>`).
#[async_trait]
pub trait JobProducerExt: JobProducer {
    /// Serialize `job` to JSON and push it onto `queue`.
    async fn push<J: Job + Serialize>(&self, queue: &str, job: J) -> anyhow::Result<()> {
        let value = serde_json::to_value(&job)?;
        self.push_json(queue, value).await
    }
}

impl<T: JobProducer + ?Sized> JobProducerExt for T {}
