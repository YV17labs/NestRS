use nest_rs::authn::JwtStrategy;

use super::Claims;

pub type AuthnStrategy = JwtStrategy<Claims>;

pub type AuthnGuard = nest_rs::authn::AuthnGuard<AuthnStrategy>;
