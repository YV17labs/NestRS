use nest_rs::core::module;
use nest_rs::throttler::ThrottlerModule;

use super::controller::AudioController;
use crate::audio::AudioModule;
use crate::authz::AuthzModule;

#[module(
    imports = [AudioModule, AuthzModule, ThrottlerModule],
    providers = [AudioController],
)]
pub struct AudioHttpModule;
