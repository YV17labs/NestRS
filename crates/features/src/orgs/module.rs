use nest_rs_core::module;

use super::service::OrgsService;

#[module(providers = [OrgsService])]
pub struct OrgsModule;
