//! [`GlobalPoolOperationGuard`] — the global guard pool, run in band on
//! `/graphql`.
//!
//! `/graphql` is `EdgePosture::Exempt`. Whenever the pool is non-empty this
//! runs it at the request, with a registered authz bridge run inside it, so a
//! bridge adds to the pool and never leaves a global guard off GraphQL. It
//! installs no ambient `Ability` of its own: the bridge does. The endpoint is
//! `Posture::InBand`, so a pooled `AuthnGuard` admits anonymous callers to the
//! resolver-level gates.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_graphql::{BoxFuture, GraphqlOperationGuard};
use poem::{Request, Response};

use crate::dispatch::denial_convert::denial_to_http_response;
use crate::dispatch::global_pool::GlobalPoolChain;

/// Runs the global guard pool in band per GraphQL request, the registered
/// [`GraphqlOperationGuard`] (the authz bridge) inside it, so `/graphql` stays
/// fail-secure under its `Exempt` edge posture and every pooled guard reaches
/// it.
pub struct GlobalPoolOperationGuard {
    /// The pool's guards listed before the first one the bridge covers.
    head: GlobalPoolChain,
    bridge: Option<Arc<dyn GraphqlOperationGuard>>,
    /// The rest of the pool, minus what the bridge covers.
    tail: GlobalPoolChain,
}

impl GlobalPoolOperationGuard {
    /// Resolve the global pool eagerly — the container is final at mount — with
    /// `bridge` run where the pool lists the first guard it
    /// [covers](GraphqlOperationGuard::covers), else after the pool.
    pub fn from_container(
        container: &Container,
        bridge: Option<Arc<dyn GraphqlOperationGuard>>,
    ) -> Self {
        let covers = bridge.as_deref().map_or(&[][..], |bridge| bridge.covers());
        let (head, tail) = GlobalPoolChain::resolve(container, "POST /graphql (operation)")
            .split_at_covered(covers);
        Self { head, bridge, tail }
    }

    /// The factory `use_guards_global` seeds as the GraphQL endpoint's global
    /// pool runner.
    pub fn factory(
        container: &Container,
        bridge: Option<Arc<dyn GraphqlOperationGuard>>,
    ) -> Arc<dyn GraphqlOperationGuard> {
        Arc::new(Self::from_container(container, bridge))
    }
}

impl GraphqlOperationGuard for GlobalPoolOperationGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
        Box::pin(async move {
            self.head
                .check(req)
                .await
                .map_err(denial_to_http_response)?;
            if let Some(bridge) = &self.bridge {
                bridge.before(req).await?;
            }
            self.tail.check(req).await.map_err(denial_to_http_response)
        })
    }

    fn around<'a>(&'a self, req: &'a Request, inner: BoxFuture<'a, ()>) -> BoxFuture<'a, ()> {
        match &self.bridge {
            Some(bridge) => bridge.around(req, inner),
            None => inner,
        }
    }
}
