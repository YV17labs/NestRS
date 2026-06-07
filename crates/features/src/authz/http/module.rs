use nest_rs_core::module;

use super::guard::AuthzGuard;
use crate::authz::AuthzModule;

#[module(
    imports = [AuthzModule],
    providers = [AuthzGuard],
)]
pub struct AuthzHttpModule;
