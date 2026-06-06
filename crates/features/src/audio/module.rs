use nest_rs_core::module;

use super::service::Transcoder;

#[module(providers = [Transcoder])]
pub struct AudioModule;
