use nest_rs::authz::http::AbilityGuard;

use crate::authz::AppAbility;

pub type AuthzGuard = AbilityGuard<AppAbility>;
