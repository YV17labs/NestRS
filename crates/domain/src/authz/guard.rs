use nestrs_authz_http::AbilityGuard;

use crate::authz::ability::AppAbility;

pub type AppAbilityGuard = AbilityGuard<AppAbility>;
