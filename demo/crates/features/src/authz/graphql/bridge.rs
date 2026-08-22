use nest_rs::authz::graphql::GraphqlAbilityBridge;

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;

pub type AuthzGraphqlBridge = GraphqlAbilityBridge<AuthnGuard, AuthzGuard>;
