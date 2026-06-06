use nest_rs_core::module;

use super::controller::AudioController;
use crate::authz::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [AudioController],
)]
pub struct AudioHttpModule;
