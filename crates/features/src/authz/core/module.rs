use nestrs_core::module;

use super::ability::AppAbility;
use crate::authn::AuthnCoreModule;

#[module(
    imports = [AuthnCoreModule],
    providers = [AppAbility],
)]
pub struct AuthzCoreModule;
