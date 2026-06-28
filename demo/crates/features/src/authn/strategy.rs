use nest_rs_authn::JwtStrategy;

use crate::Claims;

pub type AppJwtStrategy = JwtStrategy<Claims>;

/// The HTTP guard bound to [`AppJwtStrategy`]. Co-located with the strategy it
/// parameterizes — the same shape as each `oauth/strategies/*` variant binding
/// its own `…AuthGuard` alias next to its `Strategy`.
pub type AuthGuard = nest_rs_authn::AuthGuard<AppJwtStrategy>;
