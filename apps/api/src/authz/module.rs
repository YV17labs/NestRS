use nestrs_authz::AbilityGuard;
use nestrs_core::module;

use crate::authz::ability::AppAbility;

/// [`AbilityGuard`] bound to this app's policy — the name controllers bind, so
/// the concrete [`AppAbility`] stays out of them. The authentication guard must
/// run before it (it reads the `AuthUser` that guard attached), expressed by the
/// order in `#[use_guards(AuthGuard, AppAbilityGuard)]`.
pub type AppAbilityGuard = AbilityGuard<AppAbility>;

/// Authorization. Imported by `AppModule`; its providers register into the flat
/// container, so a controller in another module binds the guard with
/// `#[use_guards(AuthGuard, AppAbilityGuard)]`.
#[module(providers = [AppAbility, AppAbilityGuard])]
pub struct AuthzModule;
