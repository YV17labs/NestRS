//! [`SocketContext`] — the per-connection ambient-data seam.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use poem::Request;

use crate::WsReply;

/// A boxed, `Send` future — the return shape of [`SocketContext::around`],
/// keeping the seam object-safe.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Opaque per-connection state captured on upgrade and handed back to every
/// `around` call.
pub type Captured = Arc<dyn Any + Send + Sync>;

/// Per-connection ambient-data seam. Bind an impl with `providers = [MyBridge
/// as dyn SocketContext]` to carry state (an executor, an ability) from the
/// upgrade request into every message handler.
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
