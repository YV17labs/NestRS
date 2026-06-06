use nest_rs_core::module;

use super::guard::AppAbilityGuard;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [AppAbilityGuard],
)]
pub struct AuthzHttpModule;
