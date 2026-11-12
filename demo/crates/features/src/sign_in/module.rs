use nest_rs::core::module;
use nest_rs::oauth::server::OAuthServerModule;
use nest_rs::social::SocialModule;

use super::config::ClientPayload;
use super::service::SignInService;
use crate::users::UsersModule;

#[module(
    imports = [
        nest_rs::authn::AuthnModule,
        OAuthServerModule::<ClientPayload>,
        UsersModule,
        SocialModule,
    ],
    providers = [SignInService],
)]
pub struct SignInModule;
