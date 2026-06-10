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
//! as request data, so an `AuthGuard` in the pool admits anonymous callers
//! (resolver-level gates still apply) while a present bearer is verified —
//! exactly once, here.

use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_core::layer_chain::ResolvedLayer;
use nest_rs_graphql::{BoxFuture, GraphqlOperationGuard};
use poem::{Request, Response};

use crate::Guard;
use crate::dispatch::denial_convert::denial_to_http_response;
use crate::registry::GuardSpecs;

pub struct GlobalPoolOperationGuard {
    chain: Vec<ResolvedLayer<dyn Guard>>,
}

impl GlobalPoolOperationGuard {
    /// Resolve the global pool eagerly — the container is final at mount.
    pub fn from_container(container: &Container) -> Self {
        let chain = container
            .get::<GuardSpecs>()
            .map(|specs| specs.resolve_chain(container, "POST /graphql (operation)"))
            .unwrap_or_default();
        Self { chain }
    }

    /// The factory `use_guards_global` seeds as
    /// [`FallbackOperationGuard`](nest_rs_graphql::FallbackOperationGuard).
    pub fn factory(container: &Container) -> Arc<dyn GraphqlOperationGuard> {
        Arc::new(Self::from_container(container))
    }
}

impl GraphqlOperationGuard for GlobalPoolOperationGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
        Box::pin(async move {
            for entry in &self.chain {
                if let Err(denial) = entry.layer.check_http(req).await {
                    tracing::warn!(
                        target: "nest_rs::layers",
                        guard = entry.name,
                        reason = denial.message(),
                        "graphql operation denied by the global guard pool",
                    );
                    return Err(denial_to_http_response(denial));
                }
            }
            Ok(())
        })
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
