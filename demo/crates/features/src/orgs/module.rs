use nest_rs::core::module;

use super::service::OrgsService;

#[module(providers = [OrgsService])]
pub struct OrgsModule;
