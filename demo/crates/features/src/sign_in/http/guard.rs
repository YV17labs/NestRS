use nest_rs::authn::AuthnGuard;
use nest_rs::oauth::server::{ClientAuthenticationGuard, ClientSecretBasicStrategy};
use nest_rs::social::SocialStrategy;

use crate::sign_in::{ClientPayload, SignInService};

pub type ClientGuard = ClientAuthenticationGuard<ClientSecretBasicStrategy<ClientPayload>>;

pub type SocialGuard = AuthnGuard<SocialStrategy<SignInService>>;
