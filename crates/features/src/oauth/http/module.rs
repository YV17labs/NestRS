use nest_rs_core::module;
use nest_rs_throttler::ThrottlerGuard;

use super::controller::OAuthController;
use crate::oauth::OAuthModule;

#[module(
    imports = [OAuthModule],
    providers = [ThrottlerGuard, OAuthController],
)]
pub struct OAuthHttpModule;
