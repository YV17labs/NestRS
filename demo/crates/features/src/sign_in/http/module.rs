use nest_rs::core::module;
use nest_rs::social::SocialStrategy;
use nest_rs::throttler::ThrottlerModule;

use super::controller::SignInController;
use super::guard::SocialGuard;
use crate::sign_in::{SignInModule, SignInService};

#[module(
    imports = [SignInModule, ThrottlerModule],
    providers = [SocialStrategy<SignInService>, SocialGuard, SignInController],
)]
pub struct SignInHttpModule;
