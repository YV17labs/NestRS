use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_authz::Ability;
use nest_rs_core::injectable;
use nest_rs_graphql::ResolverGuard;
use nest_rs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};

/// Access-graph marker + fail-closed read of the seeded `Ability` — anonymous
/// GraphQL is denied by default, mirroring the HTTP `AppAbilityGuard` posture.
#[injectable]
#[derive(Default)]
pub struct GraphqlAuthGuard;

#[async_trait]
impl ResolverGuard for GraphqlAuthGuard {
    async fn check(&self, ctx: &Context<'_>) -> Result<()> {
        match ctx.data_opt::<Arc<Ability>>() {
            Some(_) => Ok(()),
            None => {
                Err(Error::new("unauthenticated")
                    .extend_with(|_, e| e.set("code", "UNAUTHENTICATED")))
            }
        }
    }
}
