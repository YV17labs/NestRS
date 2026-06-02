//! OAuth2 Authorization Code client and [`OAuth2Module`] DI wiring.

mod client;
mod config;
mod module;

pub use client::{Authorization, OAuth2Client};
pub use config::OAuth2Config;
pub use module::{OAuth2Module, OAuth2Setup};
