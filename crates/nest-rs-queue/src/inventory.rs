//! [`ProcessMethod`] — the link-time entry `#[processor]` submits for each
//! `#[process]` method, and the one way a backend reaches the method.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::__private::Site;
use nest_rs_core::Container;

use crate::checkpoint::CheckpointCell;
use crate::{Attempt, Capabilities, Capability, JobError, ProcessOptions};

/// The type-erased handler `#[processor]` emits for each `#[process]` method,
/// the tail of its attempt's pipeline: deserializes the job payload
/// ([`decode`]), runs its per-argument pipe, resolves the provider and calls
/// the method, its error classified by its type.
///
/// The payload is read in place from the stored record, which outlives the
/// attempt.
pub type JobHandler = fn(
    payload: &serde_json::Value,
    context: HandlerContext,
) -> Pin<Box<dyn Future<Output = Result<(), JobError>> + Send + '_>>;

/// The job a handler runs, read from `payload` by reference, so a payload
/// that does not decode is still there to say where it failed.
pub fn decode<T: serde::de::DeserializeOwned>(
    payload: &serde_json::Value,
) -> Result<T, nest_rs_core::DecodeError> {
    T::deserialize(payload)
        .map_err(|error| nest_rs_core::DecodeError::new(&error).located::<T, _>(payload))
}

/// What one attempt hands the handler besides the payload.
pub struct HandlerContext {
    /// The app's container, which the handler resolves its provider from.
    pub container: Container,
    /// The delivery's checkpoint, when its backend keeps one.
    pub checkpoints: Option<Arc<CheckpointCell>>,
    /// The attempt this is: the job's id, its number and its queue.
    pub attempt: Attempt,
}

/// A `#[process]` method, as its decorator declared it.
///
/// The port's [`QueueWorker`](crate::QueueWorker) drains these at boot,
/// module-gated, refusing a declaration the backend cannot honour, and runs
/// each job's attempt.
pub struct ProcessMethod {
    origin: &'static str,
    name: &'static str,
    queue: &'static str,
    options: ProcessOptions,
    provider_type_id: fn() -> TypeId,
    site: &'static Site,
    handler: JobHandler,
}

impl ProcessMethod {
    /// `module_path!()` of the crate that declared the method — read by
    /// [`inert_host`](::nest_rs_core::inert_host) to pick a report level, and
    /// emitted as a field so a skip line names a type the developer can find.
    pub const fn origin(&self) -> &'static str {
        self.origin
    }

    /// `Provider::method`, as boot lines and job spans report it.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The name of the queue this method drains.
    pub const fn queue(&self) -> &'static str {
        self.queue
    }

    /// Everything else the method declared.
    pub const fn options(&self) -> &ProcessOptions {
        &self.options
    }

    /// `TypeId` of the host provider, which a reader gates on
    /// [`ReachableProviders`](::nest_rs_core::ReachableProviders) as the
    /// worker does.
    pub fn provider_type_id(&self) -> TypeId {
        (self.provider_type_id)()
    }

    /// The dispatch site its decorator declared: its handler and the layers
    /// bound on its method and its block.
    pub(crate) fn site(&self) -> &'static Site {
        self.site
    }

    pub(crate) fn handler(&self) -> JobHandler {
        self.handler
    }
}

/// The entry `#[processor]` submits for one `#[process]` method; a hand-built
/// entry is a test's, never an app's.
pub const fn process_method(
    origin: &'static str,
    name: &'static str,
    queue: &'static str,
    options: ProcessOptions,
    provider_type_id: fn() -> TypeId,
    site: &'static Site,
    handler: JobHandler,
) -> ProcessMethod {
    ProcessMethod {
        origin,
        name,
        queue,
        options,
        provider_type_id,
        site,
        handler,
    }
}

/// The optional capabilities `method`'s declarations need from a backend.
pub fn required_capabilities(method: &ProcessMethod) -> Capabilities {
    let mut required = Capabilities::NONE;
    if method.options.throttle().is_some() {
        required = required.with(Capability::Throttle);
    }
    if method.options.checkpoint() {
        required = required.with(Capability::Checkpoint);
    }
    required
}

impl std::fmt::Debug for ProcessMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessMethod")
            .field("name", &self.name)
            .field("queue", &self.queue)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

::nest_rs_core::inventory::collect!(ProcessMethod);

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Clip {
        file: String,
    }

    #[test]
    fn a_job_decodes_from_its_payload_in_place() {
        let payload = serde_json::json!({ "file": "song.wav" });
        let clip: Clip = super::decode(&payload).expect("decodes");
        assert_eq!(
            clip,
            Clip {
                file: "song.wav".to_owned()
            }
        );
    }

    #[test]
    fn a_payload_that_does_not_decode_says_where() {
        let payload = serde_json::json!({ "file": 51 });
        let report = super::decode::<Clip>(&payload).expect_err("no string");
        assert_eq!(report.path(), Some("file"));
        assert_eq!(
            report.to_string(),
            "invalid type: an integer, expected a string"
        );
    }
}
