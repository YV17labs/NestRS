//! Authn feature — JWT strategy + [`AuthGuard`] in `core/`. No transport
//! adapter folder: the guard is a generic type alias used by every other
//! feature's HTTP / WebSocket adapter via `#[use_guards]`, and the module
//! is imported wherever those guards are bound.

pub mod core;

pub use core::{AppJwtStrategy, AuthGuard, AuthnCoreModule};
