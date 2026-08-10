use nest_rs::core::module;

use super::controller::AudioController;
use crate::audio::AudioModule;
use crate::authz::AuthzHttpModule;

#[module(
    imports = [AudioModule, AuthzHttpModule],
    providers = [AudioController],
)]
pub struct AudioHttpModule;
