use nest_rs::core::module;

use super::strategy::{AuthnGuard, AuthnStrategy};

#[module(
    imports = [nest_rs::authn::AuthnModule],
    providers = [AuthnStrategy, AuthnGuard],
)]
pub struct AuthnModule;
