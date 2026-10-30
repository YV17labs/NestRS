use nest_rs::core::module;
use nest_rs::http::HttpModule;
use nest_rs::oauth::server::ClientSecretBasicStrategy;
use nest_rs::social::SocialStrategy;

use super::controller::SignInController;
use super::guard::{ClientGuard, SocialGuard};
use crate::sign_in::{ClientPayload, SignInModule, SignInService};

#[module(
    imports = [SignInModule, HttpModule],
    providers = [
        ClientSecretBasicStrategy<ClientPayload>,
        ClientGuard,
        SocialStrategy<SignInService>,
        SocialGuard,
        SignInController,
    ],
)]
pub struct SignInHttpModule;
