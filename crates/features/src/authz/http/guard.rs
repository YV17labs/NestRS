use nest_rs_authz::http::AbilityGuard;

use crate::authz::AppAbility;

pub type AuthzGuard = AbilityGuard<AppAbility>;
