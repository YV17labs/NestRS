//! [`GlobalPoolOperationGuard`] — the fallback `GraphqlOperationGuard`.
//!
//! `/graphql` is `EdgePosture::Exempt`. With no authz bridge registered, this
//! folds the global guard pool in-band, so a forgotten bridge never leaves
//! operations unguarded; it installs no ambient `Ability`. The endpoint carries
//! the [`Public`](nest_rs_http::Public) marker, so a pooled `AuthnGuard` admits
//! anonymous callers to the resolver-level gates.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_graphql::{BoxFuture, GraphqlOperationGuard};
use poem::{Request, Response};

use crate::dispatch::denial_convert::denial_to_http_response;
use crate::dispatch::global_pool::GlobalPoolChain;

/// Runs the global guard pool in-band per GraphQL operation — the fallback
/// [`GraphqlOperationGuard`] when no app-specific bridge is registered, so
/// `/graphql` stays fail-secure under its `Exempt` edge posture.
pub struct GlobalPoolOperationGuard {
    pool: GlobalPoolChain,
}

impl GlobalPoolOperationGuard {
    /// Resolve the global pool eagerly — the container is final at mount.
    pub fn from_container(container: &Container) -> Self {
        Self {
            pool: GlobalPoolChain::resolve(container, "POST /graphql (operation)"),
        }
    }

    /// The factory `use_guards_global` seeds as
    /// [`FallbackOperationGuard`](nest_rs_graphql::__private::FallbackOperationGuard).
    pub fn factory(container: &Container) -> Arc<dyn GraphqlOperationGuard> {
        Arc::new(Self::from_container(container))
    }
}

impl GraphqlOperationGuard for GlobalPoolOperationGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
        Box::pin(async move { self.pool.check(req).await.map_err(denial_to_http_response) })
    }

    fn around<'a>(&'a self, _req: &'a Request, inner: BoxFuture<'a, ()>) -> BoxFuture<'a, ()> {
        // Nothing ambient to install — that is the authz bridge's job.
        inner
    }
}
