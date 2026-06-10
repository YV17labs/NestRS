//! [`Strategy`], [`AuthGuard`], and ready-made generic strategies. No
//! `module.rs`: every type here is generic over a caller-chosen parameter only
//! the app knows at composition time. App-specific strategies (a custom OAuth
//! flow) live next to that app's `service.rs`, not here.

mod extractors;
mod guard;
mod strategies;
mod strategy;

pub use extractors::{basic_credentials, bearer_token};
pub use guard::AuthGuard;
pub use strategies::JwtStrategy;
pub use strategy::Strategy;
