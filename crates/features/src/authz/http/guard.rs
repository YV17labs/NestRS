use nest_rs_authz::http::AbilityGuard;

use crate::authz::AppAbility;

pub type AppAbilityGuard = AbilityGuard<AppAbility>;
