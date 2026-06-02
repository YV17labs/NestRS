use nestrs_core::module;
use nestrs_throttler::ThrottlerGuard;

use super::controller::OAuthController;
use crate::oauth::core::OAuthCoreModule;

#[module(
    imports = [OAuthCoreModule],
    providers = [ThrottlerGuard, OAuthController],
)]
pub struct OAuthHttpModule;
