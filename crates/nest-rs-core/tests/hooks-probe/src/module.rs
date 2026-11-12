use nest_rs::core::module;

use crate::HooksProbe;

/// The module listing [`HooksProbe`].
#[module(providers = [HooksProbe])]
pub struct HooksProbeModule;
