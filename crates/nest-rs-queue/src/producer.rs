//! Producer seam.
//!
//! Every queue backend exposes a [`JobProducer`] surface so any feature can
//! enqueue without naming the backend. The first-class backend is `apalis-redis`
//! (shipped as `nest-rs-redis`); third-party backends provide their own
//! `*Module` that registers a `JobProducer` in the container the same way, plus
//! a `Transport` that drains [`ProcessMethod`](crate::ProcessMethod).

use async_trait::async_trait;
use serde::Serialize;

use crate::error::QueueError;
use crate::processor::Job;
use crate::queue_name::QueueName;

/// Backend-agnostic producer: push a JSON-serialized job onto a named queue.
/// Concrete backends implement this; typed pushes are a convenience built on
/// top via [`JobProducerExt`].
#[async_trait]
pub trait JobProducer: Send + Sync + 'static {
    /// Push a JSON-encoded job onto `queue`. The wire format is always JSON;
    /// a backend may re-encode internally but must round-trip the payload. A
    /// backend failure surfaces as [`QueueError::Backend`].
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> Result<(), QueueError>;
}

/// Typed-push convenience over any [`JobProducer`]. Lives as an extension trait
/// so the producer trait stays object-safe (`Arc<dyn JobProducer>`).
#[async_trait]
pub trait JobProducerExt: JobProducer {
    /// Push a job onto a **typed** queue handle — the default enqueue path. The
    /// queue name and the payload type are both taken from `Q`
    /// ([`QueueName::NAME`] and [`QueueName::Job`]), so the compiler rejects an
    /// enqueue onto the wrong queue or with the wrong payload before it ever
    /// runs. Declare `Q` once at the feature port with the
    /// [`queue`](crate::queue) macro; both the producer here and the consumer's
    /// `#[process(queue = Q)]` name the same type.
    ///
    /// Fails with [`QueueError::Serialize`] if the job won't serialize, else
    /// with whatever [`push_json`](JobProducer::push_json) returns.
    ///
    /// Passing a job of the wrong type is a compile error, not a runtime
    /// surprise:
    ///
    /// ```compile_fail
    /// use nest_rs_queue::{queue, JobProducer, JobProducerExt};
    ///
    /// #[queue(name = "transcode", job = String)]
    /// struct TranscodeQueue;
    ///
    /// async fn demo<P: JobProducer>(producer: &P) {
    ///     // `TranscodeQueue::Job` is `String`; a `u32` does not compile.
    ///     producer.push_to::<TranscodeQueue>(42u32).await.unwrap();
    /// }
    /// ```
    async fn push_to<Q: QueueName>(&self, job: Q::Job) -> Result<(), QueueError> {
        let value = serde_json::to_value(&job)?;
        self.push_json(Q::NAME, value).await
    }

    /// Push a job onto a queue named by a raw string — the dynamic-name escape
    /// hatch. Prefer [`push_to`](JobProducerExt::push_to): a typed handle
    /// compile-checks both the name and the payload type. Reach for this only
    /// when the queue name genuinely isn't known until runtime. Fails with
    /// [`QueueError::Serialize`] if the job won't serialize, else with whatever
    /// [`push_json`](JobProducer::push_json) returns.
    async fn push<J: Job + Serialize>(&self, queue: &str, job: J) -> Result<(), QueueError> {
        let value = serde_json::to_value(&job)?;
        self.push_json(queue, value).await
    }
}

impl<T: JobProducer + ?Sized> JobProducerExt for T {}
