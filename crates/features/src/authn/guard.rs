use super::strategy::AppJwtStrategy;

pub type AuthGuard = nest_rs_authn::AuthGuard<AppJwtStrategy>;
