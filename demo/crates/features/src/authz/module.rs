use nest_rs::core::module;

use super::ability::AppAbility;
use crate::authn::AuthnModule;

#[module(
    imports = [AuthnModule],
    providers = [AppAbility],
)]
pub struct AuthzModule;
