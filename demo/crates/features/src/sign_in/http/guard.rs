use nest_rs::authn::AuthnGuard;
use nest_rs::social::SocialStrategy;

use crate::sign_in::SignInService;

pub type SocialGuard = AuthnGuard<SocialStrategy<SignInService>>;
