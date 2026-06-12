use nest_rs_core::module;

use super::strategy::{AppJwtStrategy, AuthGuard};

#[module(
    imports = [nest_rs_authn::AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AuthGuard],
)]
pub struct AuthnModule;
