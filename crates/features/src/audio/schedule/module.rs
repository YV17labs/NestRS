use nestrs_core::module;

use super::producer::AudioProducer;

#[module(providers = [AudioProducer])]
pub struct AudioScheduleModule;
