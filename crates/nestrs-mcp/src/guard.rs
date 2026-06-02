//! Per-operation guard the MCP endpoint runs before each streamable-HTTP request.

use std::future::Future;
use std::pin::Pin;

use poem::{Request, Result};

/// A boxed `Send` future — the object-safe currency an [`McpOperationGuard`] uses.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Authenticates an MCP HTTP request before the streamable handler runs. Bind an
/// implementor with `providers = [MyBridge as dyn McpOperationGuard]`; the endpoint
/// resolves it from the container at mount.
pub trait McpOperationGuard: Send + Sync + 'static {
    /// Reject unauthenticated requests before the MCP session handler runs.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>>;
}
