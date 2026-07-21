use nest_rs_core::module;

use super::strategy::{AppJwtStrategy, AuthnGuard};

#[module(
    imports = [nest_rs_authn::AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AuthnGuard],
)]
pub struct AuthnModule;
