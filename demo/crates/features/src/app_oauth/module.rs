use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::social::SocialModule;

use super::config::IssuerConfig;
use super::service::AppOAuthService;
use super::strategies::{
    AppOAuthGuard, AppOAuthStrategy, ClientAuthnGuard, ClientCredentialsStrategy,
};
use crate::users::UsersModule;

#[module(
    imports = [ConfigModule::for_feature::<IssuerConfig>(), UsersModule, SocialModule],
    providers = [
        AppOAuthService,
        AppOAuthStrategy,
        AppOAuthGuard,
        ClientCredentialsStrategy,
        ClientAuthnGuard,
    ],
)]
pub struct AppOAuthModule;
