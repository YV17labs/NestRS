//! What one `/mcp` request carries to the operations rmcp runs for it.
//!
//! rmcp dispatches every operation on its own task, where the request's
//! context is gone, but it injects the request's [`Parts`] into each
//! operation's `RequestContext`: the endpoint stashes an [`McpAmbient`] in
//! their extensions, and [`PropagatingHandler`](crate::PropagatingHandler)
//! re-installs it around the operation.

use nest_rs_core::TaskContext;
use nest_rs_http::RequestParts as Parts;
use rmcp::model::Extensions;

/// What the endpoint puts in the HTTP request's extensions so it survives into
/// rmcp's per-operation `RequestContext`: the request's span and context —
/// its scope, its correlation, and what its guards and layers made ambient —
/// captured once, for a task that may outlive it.
#[derive(Clone)]
pub(crate) struct McpAmbient {
    pub(crate) carried: TaskContext,
}

impl McpAmbient {
    /// What the running request carries, captured for rmcp's tasks.
    pub(crate) fn capture() -> Self {
        Self {
            carried: TaskContext::current(),
        }
    }

    /// Read the ambient state back out of the `Parts` rmcp injects into every
    /// operation's context; `None` for an operation no endpoint request
    /// carried.
    pub(crate) fn from_extensions(extensions: &Extensions) -> Option<Self> {
        extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<Self>())
            .cloned()
    }
}
