use nestrs_core::module;

use super::guard::AppAbilityGuard;
use crate::authz::core::AuthzCoreModule;

#[module(
    imports = [AuthzCoreModule],
    providers = [AppAbilityGuard],
)]
pub struct AuthzHttpModule;
