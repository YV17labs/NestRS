//! Per-operation guard the MCP endpoint runs before each streamable-HTTP request.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::Container;
use poem::{Request, Result};

use crate::context::{Captured, OperationOutcome};

/// A boxed, `Send` future — the return type of an async guard method in a
/// dyn-compatible trait.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Authenticates an MCP HTTP request before the streamable handler runs. Bind
/// with `providers = [MyBridge as dyn McpOperationGuard]`.
///
/// A registered guard replaces [`FallbackMcpGuard`] and owns the endpoint's
/// chain. It gates the HTTP request, and never stands in for an operation's own
/// `Guard::check_mcp` chain.
pub trait McpOperationGuard: Send + Sync + 'static {
    /// Gate the operation: inspect/mutate `req` and return `Err` to reject it
    /// before the handler runs.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>>;

    /// Snapshot what [`around`](Self::around) will need, from the post-`before`
    /// request. `None` (the default) means this guard installs nothing and
    /// `around` is never called for it.
    fn capture(&self, _req: &Request) -> Option<Captured> {
        None
    }

    /// Wrap one operation's dispatch to install ambient state for its duration
    /// (the caller's `Ability`), inside rmcp's spawned dispatch, on whatever
    /// [`capture`](Self::capture) returned. Default = pass-through.
    fn around<'a>(
        &'a self,
        _captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        inner
    }
}

/// Factory slot for the fallback [`McpOperationGuard`], running the global guard
/// pool in band; a fn pointer, as `use_guards_global` seeds it before the container exists.
pub struct FallbackMcpGuard(pub fn(&Container) -> Arc<dyn McpOperationGuard>);
