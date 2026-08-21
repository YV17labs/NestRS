use nest_rs::authz::http::AbilityGuard;

use crate::app_authz::AppAbility;

pub type AppAuthzGuard = AbilityGuard<AppAbility>;
