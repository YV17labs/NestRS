use nest_rs::core::module;

use super::strategy::{AppJwtStrategy, AuthnGuard};

#[module(
    imports = [nest_rs::authn::AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AuthnGuard],
)]
pub struct AuthnModule;
