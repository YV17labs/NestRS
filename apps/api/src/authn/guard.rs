use crate::authn::strategy::AppJwtStrategy;

pub type AuthGuard = nestrs_authn::AuthGuard<AppJwtStrategy>;
