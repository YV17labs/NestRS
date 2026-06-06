//! JWT signing/verification and [`AuthnModule`] DI wiring.

mod config;
mod module;
mod service;

pub use config::JwtConfig;
pub use module::{AuthnModule, AuthnSetup};
pub use service::{JwtKey, JwtOptions, JwtService};
