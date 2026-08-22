use nest_rs::core::module;

use super::ability::AuthzAbility;
use super::guard::AuthzGuard;
use crate::authn::AuthnModule;

#[module(
    imports = [AuthnModule],
    providers = [AuthzAbility, AuthzGuard],
)]
pub struct AuthzModule;
