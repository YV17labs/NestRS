use nestrs_core::module;

use crate::orgs::controller::OrgsController;
use crate::orgs::resolver::OrgsResolver;

#[module(
    imports = [domain::orgs::OrgsModule, domain::authz::AuthzModule],
    providers = [OrgsController, OrgsResolver]
)]
pub struct OrgsModule;
