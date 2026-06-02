use nestrs_core::module;

use super::service::OrgsService;

#[module(providers = [OrgsService])]
pub struct OrgsCoreModule;
