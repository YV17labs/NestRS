//! Authn core — the JWT strategy aliased to the product's
//! [`Claims`](crate::Claims), the [`AuthGuard`] HTTP guard built on top, and
//! the module that registers both. Transport-neutral; the guard is bound by
//! every feature's HTTP / WebSocket adapter via `#[use_guards]`.

mod guard;
mod module;
mod strategy;

pub use guard::AuthGuard;
pub use module::AuthnCoreModule;
pub use strategy::AppJwtStrategy;
