use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::social::SocialModule;

use super::config::IssuerConfig;
use super::service::OAuthService;
use super::strategies::{ClientAuthnGuard, ClientCredentialsStrategy, OAuthGuard, OAuthStrategy};
use crate::users::UsersModule;

#[module(
    imports = [ConfigModule::for_feature::<IssuerConfig>(), UsersModule, SocialModule],
    providers = [
        OAuthService,
        OAuthStrategy,
        OAuthGuard,
        ClientCredentialsStrategy,
        ClientAuthnGuard,
    ],
)]
pub struct OAuthModule;
