//! Token issuance — strategies turn a provider login (or client credentials) into
//! [`Claims`](crate::Claims); [`TokenIssuer`] signs them. The `auth` app imports
//! [`OAuthModule`] and exposes the HTTP endpoints.

mod config;
mod dto;
mod error;
mod http;
mod module;
mod scope;
mod service;
mod strategy;

pub use config::{IssuerConfig, RegisteredClient};
pub use dto::LoginInput;
pub use error::TokenError;
pub use module::OAuthModule;
pub use scope::{role_from_db, roles_for_scope};
pub use service::{AccessToken, TokenIssuer};
pub use strategy::{
    AuthenticatedClient, Caller, ClientAuthGuard, ClientCredentialsStrategy, OAuthGuard,
    OAuthStrategy,
};
