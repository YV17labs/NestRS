use nestrs_authz::graphql::GraphqlAbilityBridge;

use crate::authn::AuthGuard;
use crate::authz::http::AppAbilityGuard;

pub type AppGraphqlGuard = GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>;
