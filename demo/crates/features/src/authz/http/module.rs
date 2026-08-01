use nest_rs::core::module;

use super::guard::AuthzGuard;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [AuthzGuard],
)]
pub struct AuthzHttpModule;
