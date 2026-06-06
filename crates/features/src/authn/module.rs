use nest_rs_core::module;

use super::guard::AuthGuard;
use super::strategy::AppJwtStrategy;

#[module(
    imports = [nest_rs_authn::AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AuthGuard],
)]
pub struct AuthnModule;
