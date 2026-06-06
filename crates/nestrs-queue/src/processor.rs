//! Job + Processor: the user-facing types every backend agrees on.

use async_trait::async_trait;
use nestrs_core::Container;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// JSON-round-trippable + Clone (retry keeps a copy) + cross-task safe.
///
/// The marker is intentionally minimal — backends communicate jobs as JSON on
/// the wire (the open contract), so every `Job` is `Serialize +
/// DeserializeOwned`. Backends that prefer a binary codec internally still
/// negotiate this JSON shape at the macro/inventory boundary.
pub trait Job: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}
impl<T> Job for T where T: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}

/// A returned `Err` marks the job failed; the backend retries up to the
/// `#[process(retries = N)]` budget.
#[async_trait]
pub trait Processor: Send + Sync + 'static {
    type Job: Job;

    async fn process(&self, job: Self::Job) -> anyhow::Result<()>;
}

/// Queue analog of `#[injectable]`'s `from_container`, expressed as a trait so
/// a backend can build any processor generically from the container.
pub trait FromContainer: Sized {
    fn from_container(container: &Container) -> Self;
}
