//! [`GraphqlAbilityBridge`] — per-operation bridge that authenticates and
//! attaches the caller's ability, the GraphQL analog of `AbilityGuard` + `Authorize`.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_graphql::{BoxFuture, GraphqlOperationGuard};
use nest_rs_guards::Guard;
use nest_rs_http::__private::denial_to_http_response;
use poem::{Request, Response};

use crate::chain::run_ability_chain;

/// Runs the controllers' guard chain (`A` then `G`) on the GraphQL request; the
/// ability `G` attaches is ambient for every operation the request carries. It
/// joins the app's global guard pool, never replaces it: a pooled `A` or `G`
/// runs once, here.
#[injectable]
pub struct GraphqlAbilityBridge<A: Guard, G: Guard> {
    #[inject]
    auth: Arc<A>,
    #[inject]
    ability: Arc<G>,
}

impl<A: Guard, G: Guard> GraphqlAbilityBridge<A, G> {
    const COVERS: [TypeId; 2] = [TypeId::of::<A>(), TypeId::of::<G>()];
}

impl<A: Guard, G: Guard> GraphqlOperationGuard for GraphqlAbilityBridge<A, G> {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
        Box::pin(async move {
            run_ability_chain(
                &nest_rs_graphql::__private::ENDPOINT,
                &*self.auth,
                &*self.ability,
                req,
            )
            .await
            .map_err(denial_to_http_response)
        })
    }

    fn covers(&self) -> &[TypeId] {
        &Self::COVERS
    }
}
