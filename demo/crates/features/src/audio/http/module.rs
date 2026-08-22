use nest_rs::core::module;

use super::controller::AudioController;
use crate::audio::AudioModule;
use crate::authz::AuthzModule;

#[module(
    imports = [AudioModule, AuthzModule],
    providers = [AudioController],
)]
pub struct AudioHttpModule;
