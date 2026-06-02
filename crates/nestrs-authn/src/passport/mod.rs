//! [`Strategy`], [`AuthGuard`], and ready-made strategies.

mod credentials;
mod guard;
mod jwt_strategy;
mod strategy;

pub use credentials::{basic_credentials, bearer_token};
pub use guard::AuthGuard;
pub use jwt_strategy::JwtStrategy;
pub use strategy::{Outcome, Strategy};
