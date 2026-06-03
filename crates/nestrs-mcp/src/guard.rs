//! Per-operation guard the MCP endpoint runs before each streamable-HTTP request.

use std::future::Future;
use std::pin::Pin;

use poem::{Request, Result};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Authenticates an MCP HTTP request before the streamable handler runs. Bind
/// with `providers = [MyBridge as dyn McpOperationGuard]`.
pub trait McpOperationGuard: Send + Sync + 'static {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>>;
}
