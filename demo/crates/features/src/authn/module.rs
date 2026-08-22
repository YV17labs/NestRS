use nest_rs::core::module;

use super::strategy::{AuthnGuard, AuthnStrategy};

#[module(
    imports = [nest_rs::authn::AuthnModule::for_root(None)],
    providers = [AuthnStrategy, AuthnGuard],
)]
pub struct AuthnModule;
