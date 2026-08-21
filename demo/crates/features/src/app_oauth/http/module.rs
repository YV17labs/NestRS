use nest_rs::core::module;

use super::controller::AppOAuthController;
use crate::app_oauth::AppOAuthModule;

#[module(
    imports = [AppOAuthModule],
    providers = [AppOAuthController],
)]
pub struct AppOAuthHttpModule;
