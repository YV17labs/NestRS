//! [`QueueBackend`] — who a queue backend is, and what it supports.

use crate::{Capabilities, QueueError};

/// The remedy the boot names when two queue backends bind the queue port —
/// shared with every backend's binding, so the two halves of the rule cannot
/// drift.
pub const BACKEND_REMEDY: &str = "Import exactly one queue backend's bindings — \
     `nest_rs::redis::RedisQueueModule` binds the producer over Redis.";

/// A queue backend's name and optional capabilities, declared once as a
/// constant and read by every site that refuses a declaration the backend
/// cannot honour.
///
/// The name is the backend's `messaging.system` on every job span, spelled as
/// OpenTelemetry's messaging conventions spell a system: lowercase (`"redis"`).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct QueueBackend {
    name: &'static str,
    capabilities: Capabilities,
}

impl QueueBackend {
    /// A backend named `name`, honouring `capabilities`.
    pub const fn new(name: &'static str, capabilities: Capabilities) -> Self {
        Self { name, capabilities }
    }

    /// The backend's name, as its job spans report it.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// What the backend honours beyond the contract every backend owes.
    pub const fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// Refuse the first capability of `required` this backend does not
    /// declare, with [`QueueError::Unsupported`].
    ///
    /// `pub(crate)`: the port refuses a declaration before any backend sees it,
    /// so a driver never has a question to ask here.
    pub(crate) fn check(&self, required: Capabilities) -> Result<(), QueueError> {
        match required
            .iter()
            .find(|capability| !self.capabilities.contains(*capability))
        {
            Some(capability) => Err(QueueError::Unsupported {
                capability,
                backend: self.name,
            }),
            None => Ok(()),
        }
    }
}
