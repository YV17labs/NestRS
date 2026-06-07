use nest_rs_authz::graphql::GraphqlAbilityBridge;

use crate::authn::AuthGuard;
use crate::authz::http::AuthzGuard;

pub type AppGraphqlGuard = GraphqlAbilityBridge<AuthGuard, AuthzGuard>;
