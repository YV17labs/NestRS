use nestrs_authz::graphql::GraphqlAbilityBridge;

use crate::authn::AuthGuard;
use crate::authz::http::AppAbilityGuard;

/// Runs the HTTP guard chain (`AuthGuard` then `AppAbilityGuard`) around each
/// GraphQL operation via [`GraphqlAbilityBridge`], so a query / mutation
/// enforces the same authn + ability check a REST controller does.
pub type AppGraphqlGuard = GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>;
