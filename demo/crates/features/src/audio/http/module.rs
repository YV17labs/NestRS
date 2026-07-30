use nest_rs_core::module;

use super::controller::AudioController;
use super::guard::TranscodeGuard;
use crate::audio::AudioModule;
use crate::authz::AuthzHttpModule;

#[module(
    imports = [AudioModule, AuthzHttpModule],
    providers = [AudioController, TranscodeGuard],
)]
pub struct AudioHttpModule;
