use nest_rs::core::module;

use super::controller::OAuthController;
use crate::oauth::OAuthModule;

#[module(
    imports = [OAuthModule],
    providers = [OAuthController],
)]
pub struct OAuthHttpModule;
