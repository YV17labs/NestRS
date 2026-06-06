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

/// Type-erased async job handler the `#[processor]` macro emits for each
/// `#[process]` method. Backends invoke it with a JSON payload pulled off
/// their wire; the closure deserializes to the user's job type, resolves the
/// provider from the container, and dispatches.
pub type JobHandler = fn(
    payload: serde_json::Value,
    container: Container,
) -> Pin<
    Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>,
>;

/// Runtime metadata for one consumer, surfaced via `DiscoveryService` for the
/// classic struct-level `#[processor]` form. New code uses [`ProcessMethod`]
/// directly; this type remains for backends that consume `ProcessorMeta` via
/// `DiscoveryService::meta::<ProcessorMeta>()`.
pub struct ProcessorMeta {
    pub name: &'static str,
    pub queue: &'static str,
    pub concurrency: usize,
    pub retries: usize,
    /// The type-erased handler the backend dispatches each job through.
    pub handler: JobHandler,
}

/// Link-time inventory entry submitted by `#[processor]` for each
/// `#[process]`-tagged method. A `JobConsumer` drains this registry at boot
/// and filters by
/// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so a method on a
/// provider not reachable from the app's module tree is silently skipped.
pub struct ProcessMethod {
    pub name: &'static str,
    pub queue: &'static str,
    pub concurrency: usize,
    pub retries: usize,
    pub provider_type_id: fn() -> TypeId,
    pub handler: JobHandler,
}

::nest_rs_core::inventory::collect!(ProcessMethod);
