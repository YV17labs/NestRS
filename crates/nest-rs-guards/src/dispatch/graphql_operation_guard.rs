//! [`GlobalPoolOperationGuard`] — the fallback `GraphqlOperationGuard`.
//!
//! `/graphql` is `EdgePosture::Exempt`: no guard runs at the HTTP edge, the
//! per-operation seam is the only gate. An app normally registers its authz
//! bridge there (`AppGraphqlGuard as dyn GraphqlOperationGuard`); when it
//! does not, this fallback folds the **global guard pool** in-band so a
//! forgotten bridge module never leaves GraphQL operations unguarded —
//! the fail-secure net, not the full authz integration (it installs no
//! ambient `Ability`; row scoping and masking still require the bridge).
//!
//! The GraphQL endpoint carries the [`Public`](nest_rs_core::Public) marker
//! as request data, so an `AuthnGuard` in the pool admits anonymous callers
//! (resolver-level gates still apply) while a present bearer is verified —
//! exactly once, here.

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
    /// [`FallbackOperationGuard`](nest_rs_graphql::FallbackOperationGuard).
    pub fn factory(container: &Container) -> Arc<dyn GraphqlOperationGuard> {
        Arc::new(Self::from_container(container))
    }
}

impl GraphqlOperationGuard for GlobalPoolOperationGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
        Box::pin(async move { self.pool.check(req).await.map_err(denial_to_http_response) })
    }

    fn around<'a>(
        &'a self,
        _req: &'a Request,
        inner: BoxFuture<'a, Response>,
    ) -> BoxFuture<'a, Response> {
        // Nothing ambient to install — that is the authz bridge's job.
        inner
    }
}
