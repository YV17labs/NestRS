use nest_rs::core::module;

use super::controller::OrgsController;
use crate::authz::AuthzModule;
use crate::orgs::OrgsModule;

#[module(
    imports = [OrgsModule, AuthzModule],
    providers = [OrgsController],
)]
pub struct OrgsHttpModule;
