use nest_rs_core::module;

use super::controller::OrgsController;
use crate::authz::AuthzHttpModule;
use crate::orgs::OrgsModule;

#[module(
    imports = [OrgsModule, AuthzHttpModule],
    providers = [OrgsController],
)]
pub struct OrgsHttpModule;
