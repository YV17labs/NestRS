use nestrs_config::ConfigModule;
use nestrs_core::module;

use super::config::IssuerConfig;
use super::service::{OAuthFlow, TokenIssuer};
use super::strategy::{ClientAuthGuard, ClientCredentialsStrategy, OAuthGuard, OAuthStrategy};
use crate::users::core::UsersCoreModule;

#[module(
    imports = [ConfigModule::for_feature::<IssuerConfig>(), UsersCoreModule],
    providers = [
        TokenIssuer,
        OAuthFlow,
        OAuthStrategy,
        OAuthGuard,
        ClientCredentialsStrategy,
        ClientAuthGuard,
    ],
)]
pub struct OAuthCoreModule;
