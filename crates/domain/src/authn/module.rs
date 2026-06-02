use nestrs_core::module;

use crate::authn::guard::AuthGuard;
use crate::authn::strategy::AppJwtStrategy;

#[module(
    imports = [nestrs_authn::AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AuthGuard],
)]
pub struct AuthnModule;
