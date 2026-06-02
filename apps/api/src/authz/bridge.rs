use domain::authn::AuthGuard;
use domain::authz::AppAbilityGuard;
use nestrs_authz_graphql::GraphqlAbilityBridge;

/// Runs the same guard chain as HTTP controllers (`AuthGuard` then `AppAbilityGuard`)
/// around each GraphQL operation via [`GraphqlAbilityBridge`].
pub type ApiGraphqlGuard = GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>;
