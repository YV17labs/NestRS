use nest_rs::core::module;

use super::controller::AudioController;
use crate::app_authz::AppAuthzHttpModule;
use crate::audio::AudioModule;

#[module(
    imports = [AudioModule, AppAuthzHttpModule],
    providers = [AudioController],
)]
pub struct AudioHttpModule;
