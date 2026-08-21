use nest_rs::core::module;

use super::ability::AppAbility;
use crate::app_authn::AppAuthnModule;

#[module(
    imports = [AppAuthnModule],
    providers = [AppAbility],
)]
pub struct AppAuthzModule;
