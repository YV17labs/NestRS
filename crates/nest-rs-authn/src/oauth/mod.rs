//! OAuth2 grants: the Authorization Code client (user-delegated) and the
//! client-credentials machine grant, plus [`OAuth2Module`] DI wiring.

mod client;
mod client_credentials;
mod config;
mod error;
mod module;

pub use client::{Authorization, OAuth2Client};
pub use client_credentials::{
    AuthenticatedClient, RegisteredClient, authenticate_against_registry,
};
pub use config::OAuth2Config;
pub use error::TokenError;
pub use module::{OAuth2Module, OAuth2Setup};
