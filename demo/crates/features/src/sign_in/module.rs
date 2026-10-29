use nest_rs::config::ConfigModule;
use nest_rs::core::module;
use nest_rs::oauth::server::OAuthServerModule;
use nest_rs::social::SocialModule;

use super::config::{ClientPayload, SignInConfig};
use super::service::SignInService;
use crate::users::UsersModule;

#[module(
    imports = [
        ConfigModule::for_feature::<SignInConfig>(),
        OAuthServerModule::for_root::<ClientPayload>(None),
        UsersModule,
        SocialModule,
    ],
    providers = [SignInService],
)]
pub struct SignInModule;
