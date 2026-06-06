//! [`GraphqlAbilityBridge`] — per-operation bridge that authenticates and
//! installs the ambient [`Ability`], the GraphQL analog of `AbilityGuard` +
//! `Authorize`. Implements `OperationGuard`; generic over the app's auth guard
//! `A` and ability guard `G` so the policy stays in the app.

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_graphql::{BoxFuture, OperationGuard};
use nest_rs_middleware::Guard;
use poem::{Request, Response};

use crate::{Ability, with_ability};

/// Runs the controllers' guard chain (`A` then `G`) on the GraphQL request and
/// scopes the operation to the resulting ability.
#[injectable]
pub struct GraphqlAbilityBridge<A: Guard, G: Guard> {
    #[inject]
    auth: Arc<A>,
    #[inject]
    ability: Arc<G>,
}

impl<A: Guard, G: Guard> OperationGuard for GraphqlAbilityBridge<A, G> {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // Best-effort: failed authn leaves no ability, so the resolvers'
            // `authorize`/`bind` refuse — anonymous GraphQL is closed.
            if self.auth.check(req).await.is_ok() {
                let _ = self.ability.check(req).await;
            }
        })
    }

    fn around<'a>(
        &'a self,
        req: &'a Request,
        inner: BoxFuture<'a, Response>,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move {
            // No ability (anonymous) → unscoped; the resolvers' gate then refuses.
            match req.extensions().get::<Arc<Ability>>().cloned() {
                Some(ability) => with_ability(ability, inner).await,
                None => inner.await,
            }
        })
    }
}
