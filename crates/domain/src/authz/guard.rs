use nestrs_authz::http::AbilityGuard;

use crate::authz::ability::AppAbility;

pub type AppAbilityGuard = AbilityGuard<AppAbility>;
