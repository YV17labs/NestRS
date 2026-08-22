use nest_rs::authz::AbilityGuard;

use crate::authz::AuthzAbility;

pub type AuthzGuard = AbilityGuard<AuthzAbility>;
