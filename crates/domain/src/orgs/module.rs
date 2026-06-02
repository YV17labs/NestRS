use nestrs_core::module;

use crate::orgs::resolver::OrgsResolver;
use crate::orgs::service::OrgsService;

#[module(providers = [OrgsService, OrgsResolver])]
pub struct OrgsModule;
