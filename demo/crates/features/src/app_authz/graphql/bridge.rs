use nest_rs::authz::graphql::GraphqlAbilityBridge;

use crate::app_authn::AppAuthnGuard;
use crate::app_authz::http::AppAuthzGuard;

pub type AppGraphqlGuard = GraphqlAbilityBridge<AppAuthnGuard, AppAuthzGuard>;
