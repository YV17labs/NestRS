//! [`ProcessMethod`] — the link-time entry `#[processor]` submits for each
//! `#[process]` method, and the one way a backend reaches the method.

use std::any::TypeId;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};

use crate::checkpoint::CheckpointCell;
use crate::{Attempt, Capabilities, Capability, JobError, ProcessOptions};

/// The type-erased handler `#[processor]` emits for each `#[process]` method:
/// deserializes the job payload ([`decode`]), resolves the provider, runs the
/// method as one unit of work through the bound `UnitOfWork`.
///
/// The payload is borrowed from the stored record while another attempt may
/// follow, and handed over on the last.
pub type JobHandler = fn(
    payload: Cow<'_, serde_json::Value>,
    context: HandlerContext,
) -> Pin<Box<dyn Future<Output = Result<(), JobError>> + Send + '_>>;

/// The job a handler runs, out of `payload`: moved out of it when the attempt
/// was handed the value, read from it when the value stays the record's.
pub fn decode<T: serde::de::DeserializeOwned>(
    payload: Cow<'_, serde_json::Value>,
) -> Result<T, serde_json::Error> {
    match payload {
        Cow::Owned(value) => serde_json::from_value(value),
        Cow::Borrowed(value) => T::deserialize(value),
    }
}

/// What one attempt hands the handler besides the payload.
pub struct HandlerContext {
    /// The app's container, which the handler resolves its provider from.
    pub container: Container,
    /// The delivery's checkpoint, when its backend keeps one.
    pub checkpoints: Option<Arc<CheckpointCell>>,
    /// The binding the attempt runs its unit of work through, resolved once by
    /// the worker; `None` runs it bare.
    pub unit_of_work: Option<Arc<dyn UnitOfWork>>,
    /// The attempt this is: the job's id, its number and its queue.
    pub attempt: Attempt,
}

/// Run `body` — the method's call — as the attempt's one unit of work under
/// `policy`, through the binding `context` carries: the unit names the job and
/// the attempt, and a success its driver could not honour fails the attempt
/// with the driver's classification.
pub fn run_attempt<F>(
    context: HandlerContext,
    policy: TransactionPolicy,
    body: F,
) -> impl Future<Output = Result<(), JobError>> + Send
where
    F: Future<Output = Result<(), JobError>> + Send,
{
    let HandlerContext {
        unit_of_work,
        attempt,
        ..
    } = context;
    let spec =
        UnitSpec::new(crate::unit::JOB, ExecutorScope::Job, policy).with_job(attempt.job_ref());
    async move {
        run_unit(unit_of_work.as_ref(), spec, body, Result::is_ok, |why| {
            Err(JobError::unhonoured(why))
        })
        .await
    }
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
    handler: JobHandler,
}

impl ProcessMethod {
    /// `module_path!()` of the crate that declared the method — read by
    /// [`inert_host`](::nest_rs_core::inert_host) to pick a report level, and
    /// emitted as a field so a skip line names a type the developer can find.
    pub(crate) const fn origin(&self) -> &'static str {
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

    pub(crate) fn provider_type_id(&self) -> TypeId {
        (self.provider_type_id)()
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
    handler: JobHandler,
) -> ProcessMethod {
    ProcessMethod {
        origin,
        name,
        queue,
        options,
        provider_type_id,
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
    use std::borrow::Cow;

    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Clip {
        file: String,
    }

    #[test]
    fn a_job_decodes_from_a_borrowed_payload_as_from_an_owned_one() {
        let payload = serde_json::json!({ "file": "song.wav" });
        let read: Clip = super::decode(Cow::Borrowed(&payload)).expect("decodes");
        let taken: Clip = super::decode(Cow::Owned(payload)).expect("decodes");
        assert_eq!(read, taken);
    }
}
