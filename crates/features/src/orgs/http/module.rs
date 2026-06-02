use nestrs_core::module;

use super::controller::OrgsController;
use crate::authz::AuthzHttpModule;
use crate::orgs::core::OrgsCoreModule;

#[module(
    imports = [OrgsCoreModule, AuthzHttpModule],
    providers = [OrgsController],
)]
pub struct OrgsHttpModule;
