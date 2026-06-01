use identity::Claims;
use nestrs_authn::JwtStrategy;

pub type AppJwtStrategy = JwtStrategy<Claims>;
