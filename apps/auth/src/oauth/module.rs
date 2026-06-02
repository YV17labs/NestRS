use nestrs_core::module;
use nestrs_throttler::ThrottlerGuard;

use crate::oauth::controller::OAuthController;

#[module(
    imports = [domain::oauth::OAuthModule],
    providers = [ThrottlerGuard, OAuthController],
)]
pub struct OAuthModule;
