//! [`SocketContext`] — the per-connection ambient-data seam. WS analog of
//! GraphQL's `OperationGuard`. The connection loop runs in a task *after* the
//! upgrade unwinds, so request task-locals are gone by the time a handler
//! runs. `capture` runs once on the post-guard upgrade request; `around`
//! re-installs that state per message. Bind with
//! `providers = [MyBridge as dyn SocketContext]`.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use poem::Request;

use crate::WsReply;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Opaque per-connection state captured on upgrade and handed back to every
/// `around` call.
pub type Captured = Arc<dyn Any + Send + Sync>;

pub trait SocketContext: Send + Sync + 'static {
    /// Runs once on the post-guard upgrade request. The returned state moves
    /// into the connection task.
    fn capture(&self, req: &Request) -> Captured;

    /// Wrap one message dispatch with the captured state installed.
    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, WsReply>,
    ) -> BoxFuture<'a, WsReply>;
}
