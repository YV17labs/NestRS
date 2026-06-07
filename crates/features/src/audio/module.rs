use nest_rs_core::module;

use super::service::AudioService;

#[module(providers = [AudioService])]
pub struct AudioModule;
