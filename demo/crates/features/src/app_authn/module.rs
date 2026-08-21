use nest_rs::authn::AuthnModule;
use nest_rs::core::module;

use super::strategy::{AppAuthnGuard, AppJwtStrategy};

#[module(
    imports = [AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AppAuthnGuard],
)]
pub struct AppAuthnModule;
