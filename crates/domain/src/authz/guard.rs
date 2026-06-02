use nestrs_authz_graphql::GraphqlAbilityBridge;
use nestrs_authz_http::AbilityGuard;

use crate::authn::AuthGuard;
use crate::authz::ability::AppAbility;

pub type AppAbilityGuard = AbilityGuard<AppAbility>;

pub type GraphqlAuthGuard = GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>;
