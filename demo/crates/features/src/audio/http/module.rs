use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::controller::AudioController;
use crate::audio::AudioModule;
use crate::authz::AuthzModule;

#[module(
    imports = [AudioModule, AuthzModule, HttpModule],
    providers = [AudioController],
)]
pub struct AudioHttpModule;
