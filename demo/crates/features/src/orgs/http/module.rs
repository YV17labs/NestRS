use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::controller::OrgsController;
use crate::authz::AuthzModule;
use crate::orgs::OrgsModule;

#[module(
    imports = [OrgsModule, AuthzModule, HttpModule],
    providers = [OrgsController],
)]
pub struct OrgsHttpModule;
