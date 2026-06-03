//! [`GraphqlAbilityBridge`] — the per-operation bridge that authenticates a
//! GraphQL request and installs the caller's ambient [`Ability`], the GraphQL
//! analog of [`crate::http`]'s `AbilityGuard` + `Authorize` shaper.
//!
//! It implements `nestrs-graphql`'s [`OperationGuard`] seam, so the GraphQL
//! surface stays authorization-agnostic: the endpoint resolves the bridge from
//! the container and runs it around every operation (no global interceptor —
//! non-GraphQL routes pay nothing). It is generic over the app's authentication
//! guard `A` and ability guard `G` (both ordinary [`Guard`] providers), so the
//! only app-specific parts — which strategy, which policy — stay in the app
//! behind a type alias:
//!
//! ```ignore
//! pub type AppGraphqlGuard = GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>;
//!
//! #[module(
//!     imports = [AuthzHttpModule],
//!     providers = [AppGraphqlGuard as dyn OperationGuard, /* … */],
//! )]
//! pub struct AuthzGraphqlModule;
//! ```
//!
//! In this repo the alias and the module live together in
//! `features::authz::graphql` — see `AppGraphqlGuard` and `AuthzGraphqlModule`.

use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_graphql::{BoxFuture, OperationGuard};
use nestrs_middleware::Guard;
use poem::{Request, Response};

use crate::{with_ability, Ability};

/// Runs the same guard chain the controllers use (`A` then `G`) on the GraphQL
/// request, then scopes the operation to the resulting ability. Inject it
/// generically by listing the bound alias `as dyn OperationGuard`.
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
            // Authenticate, then build the ability. Best-effort: a failed
            // authentication leaves no ability on the request, so the resolvers'
            // `authorize`/`bind` refuse the operation — anonymous GraphQL is closed.
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
            // Scope the whole operation to the caller's ability so resolver
            // `Repo` reads filter to it, exactly like the HTTP `Authorize` shaper.
            // No ability (anonymous) → run unscoped; the resolvers' gate refuses.
            match req.extensions().get::<Arc<Ability>>().cloned() {
                Some(ability) => with_ability(ability, inner).await,
                None => inner.await,
            }
        })
    }
}
