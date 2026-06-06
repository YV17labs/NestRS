use nest_rs_core::module;

use super::ability::AppAbility;
use crate::authn::AuthnModule;

#[module(
    imports = [AuthnModule],
    providers = [AppAbility],
)]
pub struct AuthzModule;
