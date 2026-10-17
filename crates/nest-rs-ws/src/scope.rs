//! Per-message request-scope bridge for WS message handlers — the WS mirror of
//! [`nest_rs_http::Scoped<T>`] and `nest_rs_mcp::Scoped<T>`.
//!
//! The dispatch loop opens a fresh [`RequestScope`](nest_rs_core::RequestScope)
//! per message through [`nest_rs_core::with_request_scope`]; a handler reads it
//! back with [`Scoped::<T>::from_context`].
//!
//! ```
//! # use std::sync::atomic::{AtomicU64, Ordering};
//! # use std::sync::Arc;
//! # use nest_rs_core::{injectable, module};
//! # use nest_rs_ws::{WsModule, gateway, messages};
//! #
//! # #[injectable]
//! # #[derive(Default)]
//! # struct Counter {
//! #     next: AtomicU64,
//! # }
//! #
//! # #[injectable(scope = request)]
//! # struct RequestSeq {
//! #     #[inject]
//! #     counter: Arc<Counter>,
//! # }
//! #
//! # impl RequestSeq {
//! #     fn value(&self) -> u64 {
//! #         self.counter.next.fetch_add(1, Ordering::SeqCst)
//! #     }
//! # }
//! #
//! # #[gateway(path = "/ws")]
//! # #[derive(Default)]
//! # struct SeqGateway;
//! #
//! # #[messages]
//! # impl SeqGateway {
//! #[subscribe_message("whoami")]
//! #[public]
//! async fn whoami(&self) -> Result<u64, nest_rs_ws::WsScopeError> {
//!     let per_msg = nest_rs_ws::Scoped::<RequestSeq>::from_context()?;
//!     Ok(per_msg.value())
//! }
//! # }
//! #
//! # #[module(imports = [WsModule], providers = [Counter, RequestSeq, SeqGateway])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let app = nest_rs_testing::TestApp::builder().module::<AppModule>().build_ws().await?;
//! # let mut socket = app.socket("/ws").connect().await;
//! # socket.send("whoami", serde_json::Value::Null).await;
//!
//! assert_eq!(socket.next_envelope().await["data"], 0);
//! # app.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Scope is **per message**, so an `#[injectable(scope = request)]` provider is
//! rebuilt for each message and shared within that one dispatch.

use std::any::type_name;
use std::ops::Deref;
use std::sync::Arc;

use crate::error::WsScopeError;

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
    /// Resolve `T` from the message's request scope. A singleton falls through
    /// (prefer plain `#[inject]` for those); a request-scoped provider is built
    /// fresh per message.
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
        nest_rs_core::with_request_scope(Some(scope), Correlation::minted(None), async {
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
            Correlation::minted(None),
            async { Scoped::<Probe>::from_context().expect("scope").0.0 },
        )
        .await;
        let second = nest_rs_core::with_request_scope(
            Some(Arc::new(RequestScope::new(container))),
            Correlation::minted(None),
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
