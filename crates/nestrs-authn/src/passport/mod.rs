//! [`Strategy`], [`AuthGuard`], and ready-made generic strategies.
//!
//! Unlike `jwt/` and `oauth/`, this folder has **no `module.rs`**: every type
//! here is generic over a caller-chosen parameter (`AuthGuard<S>`,
//! `JwtStrategy<C>`) that only the application knows at composition time.
//! Apps register the concrete instances they need directly in their own
//! `<Feature>Module`. Strategies that are *not* generic — an app's custom
//! OAuth flow, for example — live next to that app's `service.rs` /
//! `strategy.rs`, not here.

mod credentials;
mod guard;
mod strategies;
mod strategy;

pub use credentials::{basic_credentials, bearer_token};
pub use guard::AuthGuard;
pub use strategies::JwtStrategy;
pub use strategy::{Outcome, Strategy};
