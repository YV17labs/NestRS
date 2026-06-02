use nestrs_authn::JwtStrategy;

use crate::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;
