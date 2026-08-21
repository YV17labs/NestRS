use nest_rs::core::module;

use super::controller::OrgsController;
use crate::app_authz::AppAuthzHttpModule;
use crate::orgs::OrgsModule;

#[module(
    imports = [OrgsModule, AppAuthzHttpModule],
    providers = [OrgsController],
)]
pub struct OrgsHttpModule;
