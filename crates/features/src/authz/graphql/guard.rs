use std::sync::Arc;

use async_trait::async_trait;
use nestrs_authz::Ability;
use nestrs_core::injectable;
use nestrs_graphql::async_graphql::{Context, Error, Result};
use nestrs_graphql::ResolverGuard;

/// The GraphQL counterpart of HTTP's `#[use_guards(AuthGuard, AppAbilityGuard)]`.
///
/// Each `#[resolver]` binds `#[use_guards(GraphqlAuthGuard)]` so the **access
/// graph** sees the dependency on `AuthzGraphqlModule` — without this, a
/// resolver would compile fine yet panic at runtime when [`Ability`] is
/// missing from the context (the `dyn OperationGuard` bridge that builds it
/// runs from a different module, and `Ability` lives on the GraphQL context
/// rather than in the container, so the contract is otherwise invisible).
///
/// The check itself is a one-shot **read** of the seeded context: it returns
/// `Ok(())` when the `OperationGuard` bridge has installed an `Arc<Ability>`
/// (an authenticated caller) and `Err` otherwise — anonymous GraphQL is
/// closed by default, mirroring the HTTP `AppAbilityGuard` posture. Per-route
/// or per-operation policy decisions (admin-only operations, maintenance
/// gating) live in feature-specific `ResolverGuard`s that compose with this
/// one, just as a stacking HTTP guard would.
#[injectable]
#[derive(Default)]
pub struct GraphqlAuthGuard;

#[async_trait]
impl ResolverGuard for GraphqlAuthGuard {
    async fn check(&self, ctx: &Context<'_>) -> Result<()> {
        match ctx.data_opt::<Arc<Ability>>() {
            Some(_) => Ok(()),
            None => Err(Error::new(
                "unauthenticated: no ability in context — \
                 the `AuthzGraphqlModule` operation guard did not run, \
                 or the caller failed authentication",
            )),
        }
    }
}
