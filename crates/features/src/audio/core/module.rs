use nestrs_core::module;

use super::service::Transcoder;

#[module(providers = [Transcoder])]
pub struct AudioCoreModule;
