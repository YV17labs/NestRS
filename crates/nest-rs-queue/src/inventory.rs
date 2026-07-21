//! The per-method inventory entry — the link-time seam between the
//! `#[processor]` macro and any backend.
//!
//! Type-erased on purpose: the [`JobHandler`] receives a `serde_json::Value`
//! and deserializes to the method's job type inside the closure the macro
//! emits. This frees the backend from naming the user's `J` and frees the
//! inventory from carrying backend-specific function pointers.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;

use nest_rs_core::Container;

/// Wire-format version every backend wraps jobs with on push and unwraps on
/// dispatch. Bumping it lets a `#[processor]` handler reject payloads from a
/// newer release (rolling-deploy safety) instead of misinterpreting bytes.
///
/// The envelope is `{ "v": <number>, "payload": <user payload> }`. An
/// **unversioned** value — anything that isn't an object with both `v` and
/// `payload` keys — is treated as a legacy raw payload and decoded directly
/// as the job type (with a warning), so jobs left in Redis from a prior
/// deploy still drain.
pub const WIRE_FORMAT_VERSION: u32 = 1;

/// A job failure classified for the backend's retry policy (QUEUE-I4).
///
/// A **retryable** failure ([`retry`](JobError::retry)) is a transient fault —
/// the user `#[process]` method returning `Err` — that a re-attempt might clear.
/// A **non-retryable** failure ([`abort`](JobError::abort)) is *deterministic*
/// (an unsupported wire-format version, an undeserializable payload, a missing
/// provider): retrying it burns the retry budget re-failing identically before
/// the job dead-letters. A backend must abort a non-retryable failure at once
/// and surface it (an `error!` at dead-letter) instead of silently retrying.
pub struct JobError {
    /// Whether the backend's retry layer should re-attempt this job.
    pub retryable: bool,
    /// The underlying error, for logging and the backend's dead-letter record.
    pub source: Box<dyn std::error::Error + Send + Sync>,
}

impl JobError {
    /// A **retryable** failure (a transient fault worth re-attempting).
    pub fn retry(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self {
            retryable: true,
            source: source.into(),
        }
    }

    /// A **non-retryable** failure (deterministic — retrying it re-fails
    /// identically): abort and dead-letter immediately.
    pub fn abort(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self {
            retryable: false,
            source: source.into(),
        }
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, f)
    }
}

impl std::fmt::Debug for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobError")
            .field("retryable", &self.retryable)
            .field("source", &self.source)
            .finish()
    }
}

impl std::error::Error for JobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

/// Type-erased async job handler the `#[processor]` macro emits for each
/// `#[process]` method. Backends invoke it with a JSON payload pulled off
/// their wire; the closure deserializes to the user's job type, resolves the
/// provider from the container, and dispatches. A returned [`JobError`] tells
/// the backend whether the failure is worth retrying.
pub type JobHandler = fn(
    payload: serde_json::Value,
    container: Container,
) -> Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

/// Runtime metadata for one consumer, surfaced via `DiscoveryService` for the
/// classic struct-level `#[processor]` form. New code uses [`ProcessMethod`]
/// directly; this type remains for backends that consume `ProcessorMeta` via
/// `DiscoveryService::meta::<ProcessorMeta>()`.
pub struct ProcessorMeta {
    /// The processor type's name, for boot logs.
    pub name: &'static str,
    /// The queue name this consumer drains.
    pub queue: &'static str,
    /// How many jobs to process concurrently.
    pub concurrency: usize,
    /// Retry budget per job before it is considered failed.
    pub retries: usize,
    /// The type-erased handler the backend dispatches each job through.
    pub handler: JobHandler,
}

/// Link-time inventory entry submitted by `#[processor]` for each
/// `#[process]`-tagged method. A `JobConsumer` drains this registry at boot
/// and filters by
/// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so a method on a
/// provider not reachable from the app's module tree is skipped with a boot
/// `warn` (the consumer logs it, so leftover code stays visible).
pub struct ProcessMethod {
    /// The process method's name, for boot logs.
    pub name: &'static str,
    /// The queue name this method drains.
    pub queue: &'static str,
    /// How many jobs to process concurrently.
    pub concurrency: usize,
    /// Retry budget per job before it is considered failed.
    pub retries: usize,
    /// `TypeId` of the host provider, matched against the reachable set to
    /// module-gate this consumer.
    pub provider_type_id: fn() -> TypeId,
    /// The type-erased handler that resolves the provider and runs the method.
    pub handler: JobHandler,
}

::nest_rs_core::inventory::collect!(ProcessMethod);
