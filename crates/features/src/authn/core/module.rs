use nestrs_core::module;

use super::guard::AuthGuard;
use super::strategy::AppJwtStrategy;

#[module(
    imports = [nestrs_authn::AuthnModule::for_root(None)],
    providers = [AppJwtStrategy, AuthGuard],
)]
pub struct AuthnCoreModule;
