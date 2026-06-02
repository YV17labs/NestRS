use nestrs_authz::http::AbilityGuard;

use crate::authz::core::AppAbility;

pub type AppAbilityGuard = AbilityGuard<AppAbility>;
