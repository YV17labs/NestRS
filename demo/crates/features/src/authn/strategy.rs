use nest_rs_authn::JwtStrategy;

use crate::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;

pub type AuthGuard = nest_rs_authn::AuthGuard<AppJwtStrategy>;
