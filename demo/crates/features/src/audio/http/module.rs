use nest_rs_core::module;
use nest_rs_throttler::ThrottlerGuard;

use super::controller::AudioController;
use super::guard::TranscodeGuard;
use crate::audio::AudioModule;
use crate::authz::AuthzHttpModule;

#[module(
    imports = [AudioModule, AuthzHttpModule],
    providers = [AudioController, TranscodeGuard, ThrottlerGuard],
)]
pub struct AudioHttpModule;
