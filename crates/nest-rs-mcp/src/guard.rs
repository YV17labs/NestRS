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
/// With none registered the endpoint falls back to [`FallbackMcpGuard`] (the
/// global guard pool, seeded by `use_guards_global`), and with no pool either
/// to deny-all — `/mcp` is `EdgePosture::Exempt` at the HTTP edge, so this
/// in-band seam is the *only* place guards run on MCP operations. A registered
/// guard **replaces** the fallback: it owns the chain (the canonical bridge
/// runs the same `AuthnGuard` + `AuthzGuard` itself, so nothing runs twice).
pub trait McpOperationGuard: Send + Sync + 'static {
    /// Gate the operation: inspect/mutate `req` and return `Err` to reject it
    /// before the handler runs.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>>;

    /// Snapshot what [`around`](Self::around) will need, from the post-`before`
    /// request. `None` (the default) means this guard installs nothing and
    /// `around` is never called for it.
    ///
    /// The capture/`around` split is the same one
    /// [`McpToolContext`](crate::McpToolContext) uses, for the same reason: by
    /// the time an operation dispatches, rmcp has moved it onto its own task
    /// and the poem request is gone. Capturing exactly what the guard needs is
    /// what keeps that crossing off the per-request hot path.
    fn capture(&self, _req: &Request) -> Option<Captured> {
        None
    }

    /// Wrap one operation's dispatch to install ambient state for its duration
    /// (the caller's `Ability`) — the MCP twin of `GraphqlOperationGuard`'s
    /// `around`, so the *guard* installs the ability on both transports.
    ///
    /// Runs **inside** rmcp's spawned dispatch, on whatever
    /// [`capture`](Self::capture) returned. Default = pass-through.
    fn around<'a>(
        &'a self,
        _captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        inner
    }
}

/// Factory slot for the fallback [`McpOperationGuard`]. `nest-rs-guards`'
/// `use_guards_global` provides one (a fn pointer — the container does not
/// exist yet at builder time) that folds the global guard pool in-band;
/// [`resolve_operation_guard`](crate::resolve_operation_guard) invokes it at
/// mount when no `dyn McpOperationGuard` is registered. It is what lets a
/// global `ThrottlerGuard` — or the app's global authn/authz pair — reach
/// `/mcp` under its `EdgePosture::Exempt` edge.
///
/// The fallback only ever *widens* what the app explicitly opted into: with no
/// global pool the endpoint stays deny-all, and unlike `/graphql` the MCP mount
/// carries no [`Public`](nest_rs_core::Public) marker, so a pooled `AuthnGuard`
/// still refuses an unauthenticated tool call.
///
/// **Internal ABI** — a seeded fn-pointer wired by the framework crates
/// (lockstep with `nest-rs-mcp`); not a user-constructed type.
#[doc(hidden)]
pub struct FallbackMcpGuard(pub fn(&Container) -> Arc<dyn McpOperationGuard>);
