//! Per-message request-scope bridge for WS message handlers — the WS mirror of
//! [`nest_rs_http::Scoped<T>`] and `nest_rs_mcp::Scoped<T>`.
//!
//! A WS connection is a single HTTP upgrade, but each inbound message is
//! dispatched on the connection task *after* the upgrade request unwound, so
//! there is no per-message request to carry a scope through. The gateway
//! endpoint captures the singleton container once at upgrade; the dispatch loop
//! then opens a fresh [`RequestScope`](nest_rs_core::RequestScope) per message — the same per-message model
//! guards already run under — and installs it as a task-local. A handler reads
//! it back with [`Scoped::<T>::from_context`].
//!
//! ```ignore
//! #[subscribe_message("whoami")]
//! #[public]
//! async fn whoami(&self) -> Result<u64, nest_rs_ws::WsScopeError> {
//!     let per_msg = nest_rs_ws::Scoped::<RequestSeq>::from_context()?;
//!     Ok(per_msg.value())
//! }
//! ```
//!
//! Scope is **per message**, so an `#[injectable(scope = request)]` provider is
//! rebuilt for each message and shared within that one dispatch — connection is
//! not request.
//!
//! **The installer is the kernel's** ([`nest_rs_core::with_request_scope`]),
//! called by the dispatch loop. It used to be a task-local of this crate's own,
//! which made `Scoped<T>` work here while `nest_rs_core::current_trace_id()` —
//! the one accessor every other edge offers — returned `None` inside a message
//! handler. One ambient context, one accessor, every edge: a second mechanism is
//! a blind spot by construction.

use std::any::type_name;
use std::ops::Deref;
use std::sync::Arc;

/// Why a request-scoped provider could not be resolved inside a WS message
/// handler. `Display` is what the `#[messages]` reply mapping puts on the error
/// frame, so a handler can `?` it directly.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WsScopeError {
    /// The per-message request scope was not installed — the handler ran off the
    /// dispatch task, or the gateway is not nested under the HTTP request scope.
    #[error("request scope not installed — the WS gateway installs it per message")]
    NoScope,
    /// No provider of the requested type is registered in any reachable module.
    #[error("no provider registered for `{0}` — add it to a module's providers")]
    NoProvider(&'static str),
}

/// Resolves a provider of type `T` from the current WS message's
/// [`RequestScope`](nest_rs_core::RequestScope) — the per-message mirror of [`nest_rs_http::Scoped<T>`].
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
    /// Resolve `T` from the message's request scope, installed by the gateway's
    /// connection loop as a task-local for the duration of the dispatch. A
    /// singleton falls through [`RequestScope::get`](nest_rs_core::RequestScope::get) (prefer plain `#[inject]`
    /// for those); a request-scoped provider is built fresh per message.
    pub fn from_context() -> Result<Self, WsScopeError> {
        let scope = nest_rs_core::current_request_scope().ok_or(WsScopeError::NoScope)?;
        match scope.get::<T>() {
            Some(value) => Ok(Scoped(value)),
            None => Err(WsScopeError::NoProvider(type_name::<T>())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nest_rs_core::{Container, Correlation, RequestScope};

    use super::*;

    /// A request-scoped provider whose id is stamped once when the scope builds
    /// it — distinct per message, stable within one.
    struct Probe(u64);

    fn scoped_container() -> Container {
        let counter = Arc::new(AtomicU64::new(0));
        Container::builder()
            .provide_scoped::<Probe, _>(move |_| Probe(counter.fetch_add(1, Ordering::SeqCst)))
            .build()
    }

    #[tokio::test]
    async fn from_context_shares_one_instance_within_a_message() {
        let scope = Arc::new(RequestScope::new(scoped_container()));
        nest_rs_core::with_request_scope(Some(scope), Correlation::mint(), None, async {
            let a = Scoped::<Probe>::from_context().expect("scope installed");
            let b = Scoped::<Probe>::from_context().expect("scope installed");
            assert!(Arc::ptr_eq(&a.0, &b.0));
            assert_eq!(a.0.0, b.0.0);
        })
        .await;
    }

    #[tokio::test]
    async fn separate_messages_build_distinct_instances() {
        let container = scoped_container();
        let first = nest_rs_core::with_request_scope(
            Some(Arc::new(RequestScope::new(container.clone()))),
            Correlation::mint(),
            None,
            async { Scoped::<Probe>::from_context().expect("scope").0.0 },
        )
        .await;
        let second = nest_rs_core::with_request_scope(
            Some(Arc::new(RequestScope::new(container))),
            Correlation::mint(),
            None,
            async { Scoped::<Probe>::from_context().expect("scope").0.0 },
        )
        .await;
        assert_ne!(
            first, second,
            "each WS message builds its own request-scoped instance",
        );
    }

    #[tokio::test]
    async fn from_context_errors_without_an_installed_scope() {
        let err = Scoped::<Probe>::from_context()
            .map(|_| ())
            .expect_err("no scope installed");
        assert!(matches!(err, WsScopeError::NoScope));
    }
}
