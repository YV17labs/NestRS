//! Token issuance — the OAuth strategy that turns a provider login (or client
//! credentials) into this product's [`Claims`](crate::Claims), and [`TokenIssuer`]
//! that signs them into an [`AccessToken`]. The `auth` app imports [`OAuthModule`]
//! and exposes the endpoints; the logic (grant validation, scope policy, signing)
//! lives here.

mod config;
mod module;
mod service;
mod strategy;

pub use config::{IssuerConfig, RegisteredClient};
pub use module::OAuthModule;
pub use service::{AccessToken, TokenError, TokenIssuer};
pub use strategy::{
    AuthenticatedClient, Caller, ClientAuthGuard, ClientCredentialsStrategy, OAuthGuard,
    OAuthStrategy,
};
