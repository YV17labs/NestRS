//! Per-operation request-scope bridge for MCP tool methods — the MCP mirror of
//! [`nest_rs_http::Scoped<T>`].
//!
//! rmcp owns the tool-call dispatch (the handler struct is built once per
//! session, and a tool method receives no poem request), so there is no
//! parameter to forward a scope through. Instead [`GuardedEndpoint`] installs
//! the per-operation `RequestScope` as a task-local around
//! `self.inner.call(req)`, and a tool method reads it back with
//! [`Scoped::<T>::from_context`].
//!
//! ```ignore
//! #[tool(description = "…")]
//! async fn do_it(&self) -> Result<CallToolResult, McpError> {
//!     let per_op = nest_rs_mcp::Scoped::<RequestId>::from_context()?;
//!     // …
//! }
//! ```
//!
//! The scope is the one `RequestScopeEndpoint` installed outermost over the
//! whole HTTP route tree (the MCP endpoint is nested under it), so an MCP
//! operation shares the same per-request resolution model as HTTP and GraphQL.

use std::any::type_name;
use std::future::Future;
use std::ops::Deref;
use std::sync::Arc;

use nest_rs_core::RequestScope;

use crate::McpError;

tokio::task_local! {
    static MCP_REQUEST_SCOPE: Arc<RequestScope>;
}

/// Run `fut` with `scope` installed as the ambient per-operation request scope,
/// so any tool method it drives can resolve request-scoped providers via
/// [`Scoped<T>`]. Called by [`GuardedEndpoint`](crate::endpoint) around the
/// inner rmcp endpoint.
pub(crate) async fn with_request_scope<F: Future>(scope: Arc<RequestScope>, fut: F) -> F::Output {
    MCP_REQUEST_SCOPE.scope(scope, fut).await
}

/// [`with_request_scope`] for the callers that may or may not have a scope —
/// every mount path is optional here (an endpoint not nested under
/// `RequestScopeEndpoint` has none). Kept next to the task-local so the
/// "no scope ⇒ just await" branch, which decides whether [`Scoped<T>`] resolves,
/// exists once rather than at each dispatch site.
pub(crate) async fn maybe_with_request_scope<F: Future>(
    scope: Option<Arc<RequestScope>>,
    fut: F,
) -> F::Output {
    match scope {
        Some(scope) => with_request_scope(scope, fut).await,
        None => fut.await,
    }
}

/// Resolves a provider of type `T` from the current MCP operation's
/// [`RequestScope`]. `from_context` errors if the scope is absent (the endpoint
/// is not nested under `RequestScopeEndpoint`, or the tool ran off the request
/// task) or if no provider is registered for `T`.
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    /// Take the resolved provider handle out of the wrapper.
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Scoped<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Send + Sync + 'static> Scoped<T> {
    /// Resolve `T` from the operation's request scope, installed by the MCP
    /// endpoint as a task-local for the duration of the call.
    pub fn from_context() -> Result<Self, McpError> {
        let resolved = MCP_REQUEST_SCOPE
            .try_with(|scope| scope.get::<T>())
            .map_err(|_| {
                McpError::internal_error(
                    "request scope not installed — the MCP endpoint installs it per operation"
                        .to_string(),
                    None,
                )
            })?;
        match resolved {
            Some(value) => Ok(Scoped(value)),
            None => Err(McpError::internal_error(
                format!(
                    "no provider registered for `{}` — add it to a module's providers",
                    type_name::<T>()
                ),
                None,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nest_rs_core::Container;

    use super::*;

    /// A request-scoped provider whose id is stamped once when the scope builds
    /// it — distinct per operation, stable within one.
    struct Probe(u64);

    /// Container whose `Probe` factory pulls a monotonic id, so each scope that
    /// builds a `Probe` gets a fresh value.
    fn scoped_container() -> Container {
        let counter = Arc::new(AtomicU64::new(0));
        Container::builder()
            .provide_scoped::<Probe, _>(move |_| Probe(counter.fetch_add(1, Ordering::SeqCst)))
            .build()
    }

    #[tokio::test]
    async fn from_context_shares_one_instance_within_an_operation() {
        let scope = Arc::new(RequestScope::new(scoped_container()));
        with_request_scope(scope, async {
            let a = Scoped::<Probe>::from_context().expect("scope installed");
            let b = Scoped::<Probe>::from_context().expect("scope installed");
            // One `Probe` per operation: two reads resolve the same cached Arc.
            assert!(Arc::ptr_eq(&a.0, &b.0));
            assert_eq!(a.0.0, b.0.0);
        })
        .await;
    }

    #[tokio::test]
    async fn separate_operations_build_distinct_instances() {
        let container = scoped_container();
        let first = with_request_scope(Arc::new(RequestScope::new(container.clone())), async {
            Scoped::<Probe>::from_context()
                .expect("scope installed")
                .0
                .0
        })
        .await;
        let second = with_request_scope(Arc::new(RequestScope::new(container)), async {
            Scoped::<Probe>::from_context()
                .expect("scope installed")
                .0
                .0
        })
        .await;
        assert_ne!(
            first, second,
            "each MCP operation builds its own request-scoped instance",
        );
    }

    #[tokio::test]
    async fn from_context_errors_without_an_installed_scope() {
        // No `with_request_scope` wrapper — the task-local is absent, so the
        // bridge fails loudly rather than resolving from nowhere.
        let err = Scoped::<Probe>::from_context()
            .map(|_| ())
            .expect_err("no scope installed");
        assert!(
            format!("{err:?}").contains("request scope not installed"),
            "unexpected error: {err:?}",
        );
    }
}
