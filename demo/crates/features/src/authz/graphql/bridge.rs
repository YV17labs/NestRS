use nest_rs::authz::graphql::GraphqlAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::http::AuthzGuard;

pub type AppGraphqlGuard = GraphqlAbilityBridge<AuthnGuard, AuthzGuard>;
