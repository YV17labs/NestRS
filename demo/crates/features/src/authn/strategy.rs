use nest_rs_authn::JwtStrategy;

use crate::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;

pub type AuthnGuard = nest_rs_authn::AuthnGuard<AppJwtStrategy>;
