use nest_rs::core::module;

use super::guard::AppAuthzGuard;
use crate::app_authz::AppAuthzModule;

#[module(
    imports = [AppAuthzModule],
    providers = [AppAuthzGuard],
)]
pub struct AppAuthzHttpModule;
