use nest_rs::core::module;
use nest_rs::http::HttpModule;
use nest_rs::social::SocialStrategy;

use super::controller::SignInController;
use super::guard::SocialGuard;
use crate::sign_in::{SignInModule, SignInService};

#[module(
    imports = [SignInModule, HttpModule],
    providers = [SocialStrategy<SignInService>, SocialGuard, SignInController],
)]
pub struct SignInHttpModule;
