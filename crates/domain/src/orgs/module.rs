use nestrs_core::module;

use crate::orgs::resolver::OrgRelations;
use crate::orgs::service::OrgsService;

#[module(providers = [OrgsService, OrgRelations])]
pub struct OrgsModule;
