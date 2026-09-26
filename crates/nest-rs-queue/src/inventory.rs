//! [`ProcessMethod`] — the link-time entry `#[processor]` submits for each
//! `#[process]` method, and the one way a backend reaches the method.
//!
//! A backend reads what a method declares — its queue, its options, what it
//! needs from the backend — and hands each job to
//! [`consume::attempt`](crate::consume::attempt), which is the only code that
//! runs the method. The handler it calls is not part of the public surface for
//! that reason: an adapter calling it directly would skip the envelope, the
//! span, the panic catch and the classification, and nothing would say so.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::Container;

use crate::checkpoint::CheckpointCell;
use crate::{Capabilities, Capability, JobError, ProcessOptions};

/// The type-erased handler `#[processor]` emits for each `#[process]` method:
/// deserializes the job payload, resolves the provider, runs the method inside
/// the ambient `JobContext`. Internal ABI between the decorator and
/// [`consume::attempt`](crate::consume::attempt).
#[doc(hidden)]
pub type JobHandler = fn(
    payload: serde_json::Value,
    context: HandlerContext,
) -> Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

/// What one attempt hands the handler besides the payload. Internal ABI.
#[doc(hidden)]
pub struct HandlerContext {
    /// The app's container, which the handler resolves its provider from.
    pub container: Container,
    /// The delivery's checkpoint, when its backend keeps one.
    pub checkpoints: Option<Arc<CheckpointCell>>,
}

/// A `#[process]` method, as its decorator declared it.
///
/// A backend drains these at boot through
/// [`consume::discover`](crate::consume::discover), which module-gates them and
/// refuses a declaration the backend cannot honour, and runs each job through
/// [`consume::attempt`](crate::consume::attempt).
pub struct ProcessMethod {
    origin: &'static str,
    name: &'static str,
    queue: &'static str,
    options: ProcessOptions,
    provider_type_id: fn() -> TypeId,
    handler: JobHandler,
}

impl ProcessMethod {
    /// The entry `#[processor]` submits. Internal ABI: a hand-built entry is a
    /// test's, never an app's.
    #[doc(hidden)]
    pub const fn new(
        origin: &'static str,
        name: &'static str,
        queue: &'static str,
        options: ProcessOptions,
        provider_type_id: fn() -> TypeId,
        handler: JobHandler,
    ) -> Self {
        Self {
            origin,
            name,
            queue,
            options,
            provider_type_id,
            handler,
        }
    }

    /// `module_path!()` of the crate that declared the method — read by
    /// [`is_framework_owned`](::nest_rs_core::is_framework_owned) to pick a
    /// report level, and emitted as a field so a skip line names a type the
    /// developer can find.
    ///
    /// `pub(crate)`: it answers the port's own skip report. A backend is handed
    /// the methods it serves, never asked where they came from.
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

    /// The optional capabilities this method's declarations need from a
    /// backend.
    ///
    /// `#[doc(hidden)]`: the port refuses every declaration the backend does not
    /// honour before a backend sees the method, so a driver never asks. Public
    /// because `discover` and the port's own suite read it.
    #[doc(hidden)]
    pub fn required_capabilities(&self) -> Capabilities {
        let mut required = Capabilities::NONE;
        if self.options.throttle().is_some() {
            required = required.with(Capability::Throttle);
        }
        if self.options.checkpoint() {
            required = required.with(Capability::Checkpoint);
        }
        required
    }

    pub(crate) fn provider_type_id(&self) -> TypeId {
        (self.provider_type_id)()
    }

    pub(crate) fn handler(&self) -> JobHandler {
        self.handler
    }
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
