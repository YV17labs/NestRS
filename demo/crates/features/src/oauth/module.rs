use nest_rs_config::ConfigModule;
use nest_rs_core::module;
use nest_rs_social::SocialModule;

use super::config::IssuerConfig;
use super::service::OAuthService;
use super::strategies::{ClientAuthGuard, ClientCredentialsStrategy, OAuthGuard, OAuthStrategy};
use crate::users::UsersModule;

#[module(
    imports = [ConfigModule::for_feature::<IssuerConfig>(), UsersModule, SocialModule],
    providers = [
        OAuthService,
        OAuthStrategy,
        OAuthGuard,
        ClientCredentialsStrategy,
        ClientAuthGuard,
    ],
)]
pub struct OAuthModule;
