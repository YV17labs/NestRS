//! Authz HTTP adapter — [`AppAbilityGuard`] is the typed
//! `AbilityGuard<AppAbility>` controllers bind with
//! `#[use_guards(AuthGuard, AppAbilityGuard)]`.

mod guard;
mod module;

pub use guard::AppAbilityGuard;
pub use module::AuthzHttpModule;
