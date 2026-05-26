use nestrs_core::module;

use crate::authn::guard::AuthGuard;

/// Authentication. Imported by `AppModule`; [`AuthGuard`] registers into the
/// flat container, so a controller binds it with `#[use_guards(AuthGuard, ...)]`.
#[module(providers = [AuthGuard])]
pub struct AuthnModule;
