use nest_rs::authn::JwtStrategy;

use crate::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;

pub type AuthnGuard = nest_rs::authn::AuthnGuard<AppJwtStrategy>;
