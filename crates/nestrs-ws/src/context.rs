//! [`SocketContext`] — the per-connection ambient-data seam, the WebSocket
//! analog of GraphQL's `OperationGuard`.
//!
//! A gateway's connection loop runs in a task *after* the upgrade request
//! completes, so the task-locals an HTTP request installs (the ORM executor, the
//! authz ability) have already unwound by the time a message handler runs — the
//! same constraint a `#[dataloader]` batch has. This seam closes that gap without
//! `nestrs-ws` knowing anything about the ORM or authz: the crate only defines
//! the trait and resolves an optional implementor from the container; a
//! sibling module (`nestrs_database::ws`, behind the `ws` feature of
//! `nestrs-database`) implements it to re-install the executor and the caller's
//! ability around each dispatch.
//!
//! It is a two-phase hook, exactly like `OperationGuard`:
//!
//! - [`capture`](SocketContext::capture) runs **once per connection**, on the
//!   post-guard upgrade request — so the connection-level guards (`AuthGuard`,
//!   `AbilityGuard`) have already attached the principal/ability to the request
//!   extensions. It returns opaque, `Send` state moved into the connection task.
//! - [`around`](SocketContext::around) runs **per message**, wrapping the
//!   handler's dispatch with the captured ambient state installed.
//!
//! Bind an implementor with `providers = [MyBridge as dyn SocketContext]`; the
//! gateway resolves it via the container (`get_dyn`). With none registered a
//! gateway dispatches messages exactly as before — no ambient context.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use poem::Request;

use crate::WsReply;

/// A boxed `Send` future — the object-safe currency a [`SocketContext`] passes
/// the message dispatch through.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The opaque per-connection state a [`SocketContext`] captures on upgrade and
/// reads back around each dispatch. `Send + Sync` so it can be held by the
/// connection task and shared across the messages of that connection.
pub type Captured = Arc<dyn Any + Send + Sync>;

/// Per-connection ambient-data bridge — see the module docs. Surface-agnostic:
/// `nestrs-ws` defines it and runs it; a downstream crate implements it to
/// install the request executor and the caller's ability.
pub trait SocketContext: Send + Sync + 'static {
    /// Capture per-connection state from the post-guard upgrade request (the
    /// executor to bind, the ability the connection guards built). Runs once,
    /// when the socket upgrades; the returned state is moved into the connection
    /// task and handed back to every [`around`](Self::around) call.
    fn capture(&self, req: &Request) -> Captured;

    /// Wrap one message dispatch with the captured ambient state installed, so a
    /// handler's `Repo` reads run against the request executor and scope to the
    /// caller's ability — exactly like a controller.
    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, WsReply>,
    ) -> BoxFuture<'a, WsReply>;
}
