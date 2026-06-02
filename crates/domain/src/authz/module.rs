use nestrs_core::module;

use crate::authn::AuthnModule;
use crate::authz::ability::AppAbility;
use crate::authz::guard::AppAbilityGuard;

#[module(
    imports = [AuthnModule],
    providers = [AppAbility, AppAbilityGuard],
)]
pub struct AuthzModule;
